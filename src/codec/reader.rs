//! Window planning (docs/SPEC.md section 7) and output pixel conversion (section 8).

use super::header::Layout;
use super::io::FileHandle;
use super::recon::{alloc, build_levels, ColumnRun, Level};
use super::{ColorModel, Error, Result, Window};

/// Sampled columns closer together than this are computed as one run.
const RUN_GAP: u32 = 8;

/// Produces consecutive output rows of a window.
pub struct RowReader<'a> {
    levels: Vec<Level<'a>>,
    /// Current plane row for each decoded band (`[band][plane_len]`).
    plane: Vec<f32>,
    plane_len: usize,
    plane_row: Option<u32>,
    /// Position in a band's plane row of each output column.
    col_pos: Vec<usize>,
    /// `Some(start)` when output column `i` is at `start + i` in the plane row.
    contiguous: Option<usize>,
    y0: u32,
    y1: u32,
    row_step: f64,
    color: ColorModel,
    channels: usize,
    next_row: u32,
    end_row: u32,
    failed: bool,
}

impl<'a> RowReader<'a> {
    pub(crate) fn new(
        file: &'a FileHandle,
        layout: &'a Layout,
        win: Window,
        first_row: u32,
        row_count: u32,
    ) -> Result<RowReader<'a>> {
        let info = &layout.info;
        let Window {
            mut x0,
            mut y0,
            mut x1,
            mut y1,
            out_width: nx,
            out_height: ny,
        } = win;
        if x0 > x1 || x1 >= info.width || y0 > y1 || y1 >= info.height {
            return Err(Error::request("window outside the image"));
        }
        if nx == 0 || ny == 0 || nx > x1 - x0 + 1 || ny > y1 - y0 + 1 {
            return Err(Error::request("invalid output size for the window"));
        }
        if row_count == 0 || u64::from(first_row) + u64::from(row_count) > u64::from(ny) {
            return Err(Error::request("invalid row range"));
        }

        // Choose the pyramid plane.
        let mut j = 0usize;
        while u64::from(y1 - y0 + 1) > 2 * u64::from(ny)
            && u64::from(x1 - x0 + 1) > 2 * u64::from(nx)
            && j < info.stages - 1
        {
            x0 /= 2;
            x1 /= 2;
            y0 /= 2;
            y1 /= 2;
            j += 1;
        }
        if nx == 1 {
            x0 += (x1 - x0) / 2;
            x1 = x0;
        }
        if ny == 1 {
            y0 += (y1 - y0) / 2;
            y1 = y0;
        }
        let ow = x1 - x0 + 1;
        let oh = y1 - y0 + 1;

        // Output columns.
        let f = ((f64::from(ow) / f64::from(nx)) * 65536.0) as u64 as u128 * 65536;
        let mut cols: Vec<u32> = alloc(nx as usize)?;
        for (i, c) in cols.iter_mut().enumerate() {
            let off = ((i as u128 * f) >> 32).min(u128::from(ow - 1)) as u32;
            *c = x0 + off;
        }
        // Group them into runs of plane columns.
        let mut runs: Vec<ColumnRun> = Vec::new();
        let mut col_pos: Vec<usize> = alloc(nx as usize)?;
        let mut plane_len = 0usize;
        for (i, &c) in cols.iter().enumerate() {
            match runs.last_mut() {
                Some(r) if c >= r.lo && c <= r.hi.saturating_add(RUN_GAP) => {
                    if c > r.hi {
                        plane_len += (c - r.hi) as usize;
                        r.hi = c;
                    }
                }
                _ => {
                    runs.push(ColumnRun {
                        lo: c,
                        hi: c,
                        dest: plane_len,
                    });
                    plane_len += 1;
                }
            }
            let r = runs.last().ok_or_else(|| Error::request("no columns"))?;
            col_pos[i] = r.dest + (c - r.lo) as usize;
        }

        let contiguous = col_pos
            .first()
            .copied()
            .filter(|&s| col_pos.iter().enumerate().all(|(i, &p)| p == s + i));
        let channels = info.channels();
        let top = info.stages - 1 - j;
        // Size of plane G_j. With grids that are not exact halves the reduced
        // window can fall outside it; the spec leaves that undefined.
        let (pw, ph) = match layout.stages.get(top + 1) {
            Some(st) => (st.width, st.height),
            None => (info.width, info.height),
        };
        if x1 >= pw || y1 >= ph {
            return Err(Error::malformed(
                "reduced window lies outside the reduced plane (inconsistent stage sizes)",
            ));
        }
        let levels = build_levels(file, layout, top, runs, channels)?;
        let plane = alloc(
            plane_len
                .checked_mul(channels)
                .ok_or_else(|| Error::request("window too large"))?,
        )?;
        Ok(RowReader {
            levels,
            plane,
            plane_len,
            plane_row: None,
            contiguous,
            col_pos,
            y0,
            y1,
            row_step: f64::from(oh) / f64::from(ny),
            color: info.color_model,
            channels,
            next_row: first_row,
            end_row: first_row + row_count,
            failed: false,
        })
    }

    /// Write the next output row: `out_width * channels()` interleaved bytes.
    pub fn read_row(&mut self, out: &mut [u8]) -> Result<()> {
        if self.failed {
            return Err(Error::request("reader failed earlier"));
        }
        if self.next_row >= self.end_row {
            return Err(Error::request("reader is exhausted"));
        }
        let need = self.col_pos.len() * self.channels;
        if out.len() < need {
            return Err(Error::request("output buffer too short"));
        }
        let y = (f64::from(self.y0) + self.row_step * f64::from(self.next_row)) as u32;
        let y = y.clamp(self.y0, self.y1);
        if self.plane_row != Some(y) {
            self.plane_row = None;
            let Some((top, lower)) = self.levels.split_last_mut() else {
                return Err(Error::request("no levels"));
            };
            if let Err(e) = top.compute_row(y, lower, &mut self.plane, self.plane_len) {
                self.failed = true;
                return Err(e);
            }
            self.plane_row = Some(y);
        }
        self.emit(&mut out[..need]);
        self.next_row += 1;
        Ok(())
    }

    fn emit(&self, out: &mut [u8]) {
        let len = self.plane_len;
        let band = |b: usize| &self.plane[b * len..(b + 1) * len];
        let n = self.col_pos.len();
        match (self.color, self.contiguous) {
            (ColorModel::YCbCr, Some(s)) => {
                let (yb, cb, cr) = (&band(0)[s..s + n], &band(1)[s..s + n], &band(2)[s..s + n]);
                for (((px, &yv), &u), &v) in out
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .zip(yb)
                    .zip(cb)
                    .zip(cr)
                {
                    *px = ycc_to_rgb(yv, u, v);
                }
            }
            (ColorModel::YCbCr, None) => {
                let (yb, cb, cr) = (band(0), band(1), band(2));
                for (px, &p) in out.as_chunks_mut::<3>().0.iter_mut().zip(&self.col_pos) {
                    *px = ycc_to_rgb(yb[p], cb[p], cr[p]);
                }
            }
            (_, Some(s)) if self.channels == 1 => {
                for (o, &v) in out.iter_mut().zip(&band(0)[s..s + n]) {
                    *o = to_u8(v);
                }
            }
            _ => {
                let ch = self.channels;
                for c in 0..ch {
                    let src = band(c);
                    for (px, &p) in out.chunks_exact_mut(ch).zip(&self.col_pos) {
                        px[c] = to_u8(src[p]);
                    }
                }
            }
        }
    }
}

#[inline(always)]
fn ycc_to_rgb(yv: f32, u: f32, v: f32) -> [u8; 3] {
    let r = yv + 1.402f32 * v;
    let g = (yv + (-0.34414f32) * u) + (-0.71414f32) * v;
    let b = yv + 1.772f32 * u;
    [to_u8(r), to_u8(g), to_u8(b)]
}

/// `clamp(round_half_to_even(v), 0, 255)`; NaN gives 0 (via the saturating cast).
///
/// Clamping first is equivalent because rounding is monotone and the bounds
/// are integers. For `0 <= c <= 255`, adding and subtracting 2^23 rounds to
/// the nearest integer, ties to even, in IEEE arithmetic.
#[inline(always)]
fn to_u8(v: f32) -> u8 {
    const MAGIC: f32 = 8_388_608.0;
    let c = v.clamp(0.0, 255.0);
    ((c + MAGIC) - MAGIC) as u8
}
