//! Streaming, column-restricted wavelet reconstruction (docs/SPEC.md sections 5.8 and 6).
//!
//! The pyramid is evaluated lazily from the top: a [`Level`] (one stage's
//! reconstruction) produces an output row on request, pulling the two input
//! rows it depends on from its tile streams and from the level below. Only
//! the columns that eventually contribute to the requested output are
//! stored or computed. Each needed column interval is stored with clamped
//! replicas around it (an "extended" row) so the kernels never clamp.

use super::entropy::Stream;
use super::header::Layout;
use super::io::{ByteSource, FileHandle};
use super::{Error, ErrorKind, Result};

/// Gap (in columns) below which neighbouring intervals are merged and the
/// columns between them simply computed as well.
const MERGE_GAP: i64 = 4;

/// Tiles up to this size are fetched with a single read when opened.
const WHOLE_TILE_READ: u64 = 1 << 20;

fn internal(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidRequest, format!("internal error: {msg}"))
}

pub(crate) fn alloc<T: Clone + Default>(n: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n)
        .map_err(|_| Error::request("not enough memory for this window"))?;
    v.resize(n, T::default());
    Ok(v)
}

/// A requested interval of output columns `[lo, hi]` of a level, written at
/// `dest` (relative to each band's region) in the destination buffer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ColumnRun {
    pub(crate) lo: u32,
    pub(crate) hi: u32,
    pub(crate) dest: usize,
}

/// An interval of input columns `[lo, hi]`, possibly reaching one or two
/// columns beyond the plane's edges, stored at `ext` in each extended row.
#[derive(Clone, Copy, Debug)]
struct Segment {
    lo: i64,
    hi: i64,
    ext: usize,
}

impl Segment {
    /// The part of the segment inside a plane of width `w`.
    fn clamped(&self, w: u32) -> (u32, u32) {
        let lo = self.lo.max(0) as u32;
        let hi = self.hi.min(i64::from(w) - 1) as u32;
        (lo, hi)
    }
}

/// Part of a tile row copied into an extended row: `len` coefficients from
/// tile column `from` to extended position `to`.
struct Piece {
    from: usize,
    len: usize,
    to: usize,
}

/// Output positions `dest .. dest + len` whose columns lie at or beyond
/// twice the input width; `parity` is that of the first column.
struct Fill {
    dest: usize,
    len: usize,
    parity: usize,
}

struct TileColumn {
    tx: u32,
    width: u32,
    pieces: Vec<Piece>,
}

/// Supplies dequantized subband rows of one stage, for the needed columns.
struct DetailSource<'f> {
    file: &'f FileHandle,
    layout: &'f Layout,
    stage: usize,
    /// Subbands stored per band in this stage's tiles (4 for stage 0, else 3).
    subbands: usize,
    /// Bands decoded (the leading ones); further bands are skipped.
    bands: usize,
    plane_height: u32,
    tile_height: u32,
    scale: Vec<f32>,
    columns: Vec<TileColumn>,
    /// Tile row currently open, its streams (`[column][band][subband]`), and
    /// the next row within the tile row that the streams will produce.
    open_row: Option<u32>,
    streams: Vec<Stream<'f>>,
    next_local: u32,
    scratch: Vec<i16>,
}

impl<'f> DetailSource<'f> {
    fn open_tile_row(&mut self, ty: u32) -> Result<()> {
        self.open_row = None;
        self.streams.clear();
        let total_bands = self.layout.info.bands as u64;
        let n = total_bands * self.subbands as u64;
        let header_len = 4 * (n - 1);
        let used = (self.bands * self.subbands) as u64;
        for col in &self.columns {
            let (start, len) = self.layout.tile_range(self.stage, col.tx, ty)?;
            if len == 0 {
                for _ in 0..used {
                    self.streams.push(Stream::Zero);
                }
                continue;
            }
            if len < header_len {
                return Err(Error::malformed("tile shorter than its stream table"));
            }
            let whole = if len <= WHOLE_TILE_READ {
                Some(self.file.read_vec(start, len)?)
            } else {
                None
            };
            // Offsets of the used streams plus one more (for a length estimate).
            let wanted = (used + 1).min(n);
            let table = match &whole {
                Some(t) => t[..(4 * (wanted - 1)) as usize].to_vec(),
                None => self.file.read_vec(start, 4 * (wanted - 1))?,
            };
            let stream_start = |j: u64| -> Result<u64> {
                let off = if j == 0 {
                    header_len
                } else {
                    let at = (4 * (j - 1)) as usize;
                    let a = u32::from_be_bytes([
                        table[at],
                        table[at + 1],
                        table[at + 2],
                        table[at + 3],
                    ]);
                    header_len + u64::from(a)
                };
                if off > len {
                    return Err(Error::malformed("stream starts beyond its tile"));
                }
                Ok(off)
            };
            for j in 0..used {
                let s = stream_start(j)?;
                let prefix = match &whole {
                    Some(t) => {
                        let est_end = if j + 1 < n {
                            stream_start(j + 1).ok().filter(|&e| e > s).unwrap_or(len)
                        } else {
                            len
                        };
                        let end = est_end.saturating_add(16).min(len);
                        t[s as usize..end as usize].to_vec()
                    }
                    None => Vec::new(),
                };
                let src = ByteSource::new(self.file, start + s, start + len, prefix);
                self.streams.push(Stream::open(src)?);
            }
        }
        self.open_row = Some(ty);
        self.next_local = 0;
        Ok(())
    }

    /// Decode row `r` of this stage's subbands into extended row storage
    /// `dst` (layout `[band][subband 0..4][ext_len]`). Rows must be
    /// requested in increasing order.
    fn fetch(&mut self, r: u32, dst: &mut [f32], ext_len: usize) -> Result<()> {
        if r >= self.plane_height {
            return Err(internal("row outside plane"));
        }
        let ty = r / self.tile_height;
        let local = r % self.tile_height;
        if self.open_row != Some(ty) {
            self.open_tile_row(ty)?;
        }
        if local < self.next_local {
            return Err(internal("rows requested out of order"));
        }
        let per_col = self.bands * self.subbands;
        // Skip rows that are not needed but precede the wanted one.
        while self.next_local < local {
            for (ci, col) in self.columns.iter().enumerate() {
                let buf = &mut self.scratch[..col.width as usize];
                for s in &mut self.streams[ci * per_col..(ci + 1) * per_col] {
                    s.decode(buf)?;
                }
            }
            self.next_local += 1;
        }
        let first_sub = 4 - self.subbands;
        for (ci, col) in self.columns.iter().enumerate() {
            let buf = &mut self.scratch[..col.width as usize];
            let streams = &mut self.streams[ci * per_col..(ci + 1) * per_col];
            for (k, s) in streams.iter_mut().enumerate() {
                s.decode(buf)?;
                let band = k / self.subbands;
                let sub = first_sub + k % self.subbands;
                let scale = self.scale[band];
                let base = (band * 4 + sub) * ext_len;
                for p in &col.pieces {
                    let out = &mut dst[base + p.to..base + p.to + p.len];
                    let inp = &buf[p.from..p.from + p.len];
                    for (o, &c) in out.iter_mut().zip(inp) {
                        *o = f32::from(c) * scale;
                    }
                }
            }
        }
        self.next_local += 1;
        Ok(())
    }
}

/// Reconstruction of one stage, restricted to some output column runs.
pub(crate) struct Level<'f> {
    stage: usize,
    in_width: u32,
    in_height: u32,
    bands: usize,
    segments: Vec<Segment>,
    /// Length of one extended subband row (all segments).
    ext_len: usize,
    /// Output runs, the segment each reads from, and whether the run is the
    /// canonical pair of far-edge columns (written to `canon`).
    runs: Vec<(ColumnRun, usize, bool)>,
    /// Destination ranges filled from `canon` (see `build_levels`).
    fills: Vec<Fill>,
    /// Values of output columns 2w and 2w+1, per band.
    canon: Vec<f32>,
    slots: [Vec<f32>; 2],
    tags: [Option<u32>; 2],
    even: Vec<f32>,
    odd: Vec<f32>,
    details: DetailSource<'f>,
}

/// Build the levels needed to produce `runs` of the output of stage `top`.
/// Returned bottom-up: index `s` reconstructs stage `s`.
pub(crate) fn build_levels<'f>(
    file: &'f FileHandle,
    layout: &'f Layout,
    top: usize,
    runs: Vec<ColumnRun>,
    bands: usize,
) -> Result<Vec<Level<'f>>> {
    let tile_w = layout.info.tile_width;
    let mut levels = Vec::with_capacity(top + 1);
    let mut runs = runs;
    for stage in (0..=top).rev() {
        let st = &layout.stages[stage];
        let (w, h) = (st.width, st.height);

        // Output columns X >= 2w read only clamped copies of input column
        // w-1 (both ca and cb clamp), so their value depends only on the
        // parity of X. Such columns, which only occur when grids are not
        // exact halves, are computed once (as columns 2w, 2w+1) and
        // replicated, so memory stays bounded by the input width.
        let edge = 2 * u64::from(w);
        let mut fills = Vec::new();
        let mut eff: Vec<(ColumnRun, bool)> = Vec::with_capacity(runs.len() + 1);
        for r in &runs {
            if u64::from(r.hi) < edge {
                eff.push((*r, false));
                continue;
            }
            if u64::from(r.lo) < edge {
                let hi = (edge - 1) as u32;
                eff.push((ColumnRun { hi, ..*r }, false));
            }
            let start = r.lo.max(edge as u32);
            fills.push(Fill {
                dest: r.dest + (start - r.lo) as usize,
                len: (r.hi - start + 1) as usize,
                parity: (start & 1) as usize,
            });
        }
        if !fills.is_empty() {
            let lo = edge as u32;
            eff.push((
                ColumnRun {
                    lo,
                    hi: lo + 1,
                    dest: 0,
                },
                true,
            ));
        }

        // Input column intervals with one clamped neighbour on each side.
        let mut segments: Vec<Segment> = Vec::new();
        let mut run_seg = Vec::with_capacity(eff.len());
        for (r, canon) in &eff {
            // With arbitrary grid sizes the whole interval may lie beyond the
            // right edge; extend it down to the last column so the in-plane
            // part is never empty.
            let lo = (i64::from(r.lo / 2) - 1).min(i64::from(w) - 1);
            let hi = i64::from(r.hi / 2) + 1;
            match segments.last_mut() {
                Some(s) if lo <= s.hi + 1 + MERGE_GAP => s.hi = s.hi.max(hi),
                _ => segments.push(Segment { lo, hi, ext: 0 }),
            }
            run_seg.push((*r, segments.len() - 1, *canon));
        }
        let mut ext_len = 0usize;
        for s in &mut segments {
            s.ext = ext_len;
            ext_len += (s.hi - s.lo + 1) as usize;
        }

        // Which parts of which tiles feed the segments.
        let mut columns: Vec<TileColumn> = Vec::new();
        for s in &segments {
            let (cl, ch) = s.clamped(w);
            let mut c = cl;
            while c <= ch {
                let tx = c / tile_w;
                let tx0 = tx * tile_w;
                let tile_end = tx0.saturating_add(tile_w).min(w) - 1;
                let pend = tile_end.min(ch);
                let piece = Piece {
                    from: (c - tx0) as usize,
                    len: (pend - c + 1) as usize,
                    to: s.ext + (i64::from(c) - s.lo) as usize,
                };
                match columns.last_mut() {
                    Some(col) if col.tx == tx => col.pieces.push(piece),
                    _ => columns.push(TileColumn {
                        tx,
                        width: tile_end - tx0 + 1,
                        pieces: vec![piece],
                    }),
                }
                c = pend + 1;
            }
        }

        let q = f32::from(layout.quant_divisor);
        let scale = st.quant_steps[..bands]
            .iter()
            .map(|&qs| (qs as f32) / q)
            .collect();
        let slot_len = bands
            .checked_mul(4 * ext_len)
            .ok_or_else(|| Error::request("window too large"))?;
        let max_pairs = eff
            .iter()
            .map(|(r, _)| (r.hi / 2 - r.lo / 2 + 1) as usize)
            .max()
            .unwrap_or(0);
        let details = DetailSource {
            file,
            layout,
            stage,
            subbands: if stage == 0 { 4 } else { 3 },
            bands,
            plane_height: h,
            tile_height: layout.info.tile_height,
            scale,
            columns,
            open_row: None,
            streams: Vec::new(),
            next_local: 0,
            scratch: alloc(tile_w as usize)?,
        };
        // The level below must deliver the in-plane part of each segment.
        let lower_runs = segments
            .iter()
            .map(|s| {
                let (lo, hi) = s.clamped(w);
                ColumnRun {
                    lo,
                    hi,
                    dest: s.ext + (i64::from(lo) - s.lo) as usize,
                }
            })
            .collect();
        levels.push(Level {
            stage,
            in_width: w,
            in_height: h,
            bands,
            segments,
            ext_len,
            runs: run_seg,
            fills,
            canon: alloc(2 * bands)?,
            slots: [alloc(slot_len)?, alloc(slot_len)?],
            tags: [None, None],
            even: alloc(max_pairs)?,
            odd: alloc(max_pairs)?,
            details,
        });
        runs = lower_runs;
    }
    levels.reverse();
    Ok(levels)
}

impl Level<'_> {
    /// Compute output row `y` of this stage for all runs and bands, writing
    /// band `b`'s run at `dest[b * stride + run.dest ..]`. `lower` holds the
    /// levels of the coarser stages (bottom-up).
    pub(crate) fn compute_row(
        &mut self,
        y: u32,
        lower: &mut [Level<'_>],
        dest: &mut [f32],
        stride: usize,
    ) -> Result<()> {
        let last = i64::from(self.in_height) - 1;
        let y = i64::from(y);
        let (r0, r1) = if y % 2 == 0 {
            (y / 2 - 1, y / 2)
        } else {
            ((y - 1) / 2, (y - 1) / 2 + 1)
        };
        let r0 = r0.clamp(0, last) as u32;
        let r1 = r1.clamp(0, last) as u32;
        let (i0, i1) = self.ensure_rows(r0, r1, lower)?;

        let ext = self.ext_len;
        let row0 = &self.slots[i0];
        let row1 = &self.slots[i1];
        for b in 0..self.bands {
            let sub = |k: usize| (b * 4 + k) * ext..(b * 4 + k + 1) * ext;
            let a = [&row0[sub(0)], &row0[sub(1)], &row0[sub(2)], &row0[sub(3)]];
            let c = [&row1[sub(0)], &row1[sub(1)], &row1[sub(2)], &row1[sub(3)]];
            for &(run, si, to_canon) in &self.runs {
                let seg = self.segments[si];
                let n = (run.hi / 2 - run.lo / 2 + 1) as usize;
                let e0 = seg.ext + (i64::from(run.lo / 2) - 1 - seg.lo) as usize;
                let wa = a.map(|s| &s[e0..e0 + n + 2]);
                let wc = c.map(|s| &s[e0..e0 + n + 2]);
                let (ev, od) = (&mut self.even[..n], &mut self.odd[..n]);
                if y % 2 == 0 {
                    kernel_even_row(wa[1], wa[3], wc[0], wc[1], wc[2], wc[3], ev, od);
                } else {
                    kernel_odd_row(wa[0], wa[1], wa[2], wa[3], wc[0], wc[2], ev, od);
                }
                let len = (run.hi - run.lo + 1) as usize;
                let out = if to_canon {
                    &mut self.canon[b * 2..b * 2 + 2]
                } else {
                    let start = b * stride + run.dest;
                    &mut dest[start..start + len]
                };
                interleave(ev, od, (run.lo & 1) as usize, out);
            }
            let pair = [self.canon[b * 2], self.canon[b * 2 + 1]];
            for f in &self.fills {
                let start = b * stride + f.dest;
                for (k, o) in dest[start..start + f.len].iter_mut().enumerate() {
                    *o = pair[(f.parity + k) & 1];
                }
            }
        }
        Ok(())
    }

    /// Make sure input rows `r0 <= r1` are loaded; returns their slots.
    fn ensure_rows(&mut self, r0: u32, r1: u32, lower: &mut [Level<'_>]) -> Result<(usize, usize)> {
        for r in [r0, r1] {
            if self.tags.contains(&Some(r)) {
                continue;
            }
            let keep = |t: Option<u32>| t == Some(r0) || t == Some(r1);
            let victim = if keep(self.tags[0]) { 1 } else { 0 };
            self.load(victim, r, lower)?;
        }
        let find = |r: u32| self.tags.iter().position(|&t| t == Some(r));
        match (find(r0), find(r1)) {
            (Some(a), Some(b)) => Ok((a, b)),
            _ => Err(internal("row cache")),
        }
    }

    fn load(&mut self, slot: usize, r: u32, lower: &mut [Level<'_>]) -> Result<()> {
        self.tags[slot] = None;
        let ext = self.ext_len;
        let buf = &mut self.slots[slot];
        self.details.fetch(r, buf, ext)?;
        if self.stage > 0 {
            let (below, rest) = lower
                .split_last_mut()
                .ok_or_else(|| internal("missing lower level"))?;
            below.compute_row(r, rest, buf, 4 * ext)?;
        }
        // Replicate edge samples into the out-of-plane positions.
        let w = self.in_width;
        for b in 0..self.bands {
            for k in 0..4 {
                let row = &mut buf[(b * 4 + k) * ext..(b * 4 + k + 1) * ext];
                for s in &self.segments {
                    let (cl, ch) = s.clamped(w);
                    let left = (i64::from(cl) - s.lo) as usize;
                    let right = (i64::from(ch) - s.lo) as usize;
                    let full = (s.hi - s.lo) as usize;
                    let seg = &mut row[s.ext..s.ext + full + 1];
                    let lv = seg[left];
                    seg[..left].fill(lv);
                    let rv = seg[right];
                    seg[right + 1..].fill(rv);
                }
            }
        }
        self.tags[slot] = Some(r);
        Ok(())
    }
}

/// Output row with even `Y`. Inputs are extended rows starting at column
/// `n_lo - 1` (row `r0` = `a*`, row `r1` = `b*`, digit = subband), each
/// `n + 2` long; writes `n` even-column and `n` odd-column outputs.
#[allow(clippy::too_many_arguments)]
#[inline]
fn kernel_even_row(
    a1: &[f32],
    a3: &[f32],
    b0: &[f32],
    b1: &[f32],
    b2: &[f32],
    b3: &[f32],
    ev: &mut [f32],
    od: &mut [f32],
) {
    let n = ev.len().min(od.len());
    let (a1, a3, b0, b1, b2, b3) = (
        &a1[..n + 2],
        &a3[..n + 2],
        &b0[..n + 2],
        &b1[..n + 2],
        &b2[..n + 2],
        &b3[..n + 2],
    );
    let (ev, od) = (&mut ev[..n], &mut od[..n]);
    for i in 0..n {
        ev[i] = b0[i + 1] - (a1[i + 1] + b1[i + 1] + b2[i] + b2[i + 1]) * 0.5
            + (a3[i] + a3[i + 1] + b3[i] + b3[i + 1]) * 0.25;
        od[i] = ((b0[i + 1] + b0[i + 2]) - (a3[i + 1] + b3[i + 1])) * 0.5
            - (a1[i + 1] + a1[i + 2] + b1[i + 1] + b1[i + 2]) * 0.25
            + b2[i + 1];
    }
}

/// Output row with odd `Y`; see [`kernel_even_row`].
#[allow(clippy::too_many_arguments)]
#[inline]
fn kernel_odd_row(
    a0: &[f32],
    a1: &[f32],
    a2: &[f32],
    a3: &[f32],
    b0: &[f32],
    b2: &[f32],
    ev: &mut [f32],
    od: &mut [f32],
) {
    let n = ev.len().min(od.len());
    let (a0, a1, a2, a3, b0, b2) = (
        &a0[..n + 2],
        &a1[..n + 2],
        &a2[..n + 2],
        &a3[..n + 2],
        &b0[..n + 2],
        &b2[..n + 2],
    );
    let (ev, od) = (&mut ev[..n], &mut od[..n]);
    for i in 0..n {
        ev[i] = ((a0[i + 1] + b0[i + 1]) - (a3[i] + a3[i + 1])) * 0.5 + a1[i + 1]
            - (a2[i] + a2[i + 1] + b2[i] + b2[i + 1]) * 0.25;
        od[i] = (a0[i + 1] + a0[i + 2] + b0[i + 1] + b0[i + 2]) * 0.25
            + (a1[i + 1] + a1[i + 2] + a2[i + 1] + b2[i + 1]) * 0.5
            + a3[i + 1];
    }
}

/// Merge even/odd column outputs into `out`, which starts at an even
/// column if `parity == 0` and at an odd column otherwise.
#[inline]
fn interleave(ev: &[f32], od: &[f32], parity: usize, out: &mut [f32]) {
    let (mut ev, mut od, mut out) = (ev, od, out);
    if parity == 1 {
        let Some((first, rest)) = out.split_first_mut() else {
            return;
        };
        *first = od.first().copied().unwrap_or(0.0);
        out = rest;
        ev = ev.get(1..).unwrap_or(&[]);
        od = od.get(1..).unwrap_or(&[]);
    }
    let half = out.len() / 2;
    let (paired, tail) = out.split_at_mut(2 * half);
    for ((p, &e), &o) in paired.as_chunks_mut::<2>().0.iter_mut().zip(ev).zip(od) {
        *p = [e, o];
    }
    if let (Some(t), Some(&e)) = (tail.first_mut(), ev.get(half)) {
        *t = e;
    }
}
