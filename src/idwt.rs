//! Streaming inverse-DWT reconstruction of an ECW region.
//!
//! Mirrors the SDK's region decoder: each level keeps two lines (line0/line1)
//! of its four sidebands per band. An output line of level L is synthesized
//! from those two lines with a 1-2-1 filter whose taps depend on the parity of
//! the output x/y; the LL line of level L is produced recursively by level
//! L-1, while LH/HL/HH (and LL at level 0) are entropy-decoded from blocks.

use crate::entropy::SbDecoder;
use crate::header::{CompressFormat, EcwFile};

pub type Error = String;

const MAX_SIDEBAND: usize = 4;

struct XBlockInfo {
    line_length: usize, // width of this block in symbols
    pre_skip: usize,
    post_skip: usize,
    valid: usize,
}

struct XBlock {
    data: Vec<u8>,
    /// one decoder per (band, sideband >= first_sideband), band-major
    sbs: Vec<SbDecoder>,
}

#[allow(dead_code)] // some fields kept for parity with the reference decoder
struct RegionLevel {
    level: usize,
    // level (QMF) dimensions
    x_size: u32,
    y_size: u32,
    x_block_size: u32,
    y_block_size: u32,
    bin_sizes: Vec<f32>, // per band, already divided by scale_factor
    first_sideband: usize,
    bands: usize,

    reflect_start_x: usize,
    reflect_end_x: usize,
    reflect_start_y: usize,
    #[allow(unused)]
    reflect_end_y: usize,

    level_start_x: u32,
    level_end_x: u32,
    level_size_x: usize,
    level_start_y: u32,
    #[allow(unused)]
    level_end_y: u32,
    level_size_y: u32,

    output_level_start_x: u32,
    output_level_size_x: usize,

    start_x_block: u32,
    x_block_count: u32,
    block_infos: Vec<XBlockInfo>,

    // decode state
    start_line: u32,
    current_line: u32,
    read_lines: u32,
    next_input_block_y_line: u32,
    have_blocks: bool,
    blocks: Vec<XBlock>,

    // line buffers: (bands * 4) chunks of `stride` f32s
    stride: usize,
    line0: Vec<f32>,
    line1: Vec<f32>,
}

pub struct Region {
    pub number_x: u32,
    pub format: CompressFormat,
    pub bands: usize,
    levels: Vec<RegionLevel>, // index == level, 0..=largest
    // output driver state
    start_line: f64,
    current_line: f64,
    increment_x: f64,
    increment_y: f64,
    read_line: u32,
    counter: u64,
    out_w: usize,
    ll: Vec<f32>, // bands * out_w
}

impl RegionLevel {
    #[inline]
    fn chunk(&self, band: usize, sb: usize) -> std::ops::Range<usize> {
        let base = (band * MAX_SIDEBAND + sb) * self.stride;
        base..base + self.stride
    }

    fn start_blocks(&mut self, ecw: &EcwFile) -> Result<(), Error> {
        let next_y_block = self.current_line / self.y_block_size;
        let skip = self.current_line - next_y_block * self.y_block_size;
        self.next_input_block_y_line = skip;
        self.blocks.clear();
        let n_sb = MAX_SIDEBAND - self.first_sideband;
        let total_sb = self.bands * n_sb;
        for k in 0..self.x_block_count {
            let bx = self.start_x_block + k;
            let data = ecw
                .read_block(self.level, bx, next_y_block)
                .map_err(|e| format!("read block L{} ({},{}): {e}", self.level, bx, next_y_block))?;
            if data.is_empty() {
                // ECW v3 empty block: every sideband decodes to zeros.
                let sbs = (0..total_sb).map(|_| SbDecoder::Zeros).collect();
                self.blocks.push(XBlock { data, sbs });
                continue;
            }
            // Parse the sideband offset array: (total_sb - 1) u32 MSB values.
            let offs_len = 4 * (total_sb - 1);
            if data.len() < offs_len {
                return Err(format!("block too short at L{} ({},{})", self.level, bx, next_y_block));
            }
            let mut sb_offsets = Vec::with_capacity(total_sb);
            sb_offsets.push(0usize);
            for i in 0..total_sb - 1 {
                let o = u32::from_be_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
                sb_offsets.push(o as usize);
            }
            let info = &self.block_infos[k as usize];
            let mut sbs = Vec::with_capacity(total_sb);
            for (i, &sb_off) in sb_offsets.iter().enumerate() {
                let off = offs_len + sb_off;
                sbs.push(
                    SbDecoder::start(&data, off, info.line_length, skip as usize)
                        .map_err(|e| format!("sideband start L{} ({},{}) sb{}: {e}", self.level, bx, next_y_block, i))?,
                );
            }
            self.blocks.push(XBlock { data, sbs });
        }
        self.have_blocks = true;
        Ok(())
    }

    /// Decode one line of all sidebands from the current block row into line1.
    fn unpack_line(&mut self) -> Result<(), Error> {
        let n_sb = MAX_SIDEBAND - self.first_sideband;
        for k in 0..self.blocks.len() {
            let block_offset = if k == 0 {
                self.reflect_start_x
            } else {
                self.reflect_start_x
                    + self.block_infos[0].valid
                    + (k - 1) * self.x_block_size as usize
            };
            let info = &self.block_infos[k];
            let block = &mut self.blocks[k];
            for band in 0..self.bands {
                let bin_size = self.bin_sizes[band];
                for sb in self.first_sideband..MAX_SIDEBAND {
                    let dec = &mut block.sbs[band * n_sb + (sb - self.first_sideband)];
                    let base = (band * MAX_SIDEBAND + sb) * self.stride + block_offset;
                    let out = &mut self.line1[base..base + info.valid];
                    dec.decode_line(
                        &block.data,
                        info.pre_skip,
                        info.post_skip,
                        info.line_length,
                        out,
                        bin_size,
                    )
                    .map_err(|e| format!("decode line L{} block {k} band {band} sb {sb}: {e}", self.level))?;
                }
            }
        }
        Ok(())
    }

    fn read_sideband_line(&mut self, ecw: &EcwFile) -> Result<(), Error> {
        if !self.have_blocks {
            self.start_blocks(ecw)?;
        }
        self.unpack_line()?;
        self.next_input_block_y_line += 1;
        if self.next_input_block_y_line >= self.y_block_size
            || self.current_line - self.start_line >= self.level_size_y - 1
        {
            self.blocks.clear();
            self.have_blocks = false;
        }
        Ok(())
    }
}

/// Recursively produce one output line of the last level in `levels`.
/// `out[band]` receives `output_level_size_x` values.
fn pull_line(
    levels: &mut [RegionLevel],
    ecw: &EcwFile,
    y_line: u32,
    out: &mut [&mut [f32]],
) -> Result<(), Error> {
    let (smaller, cur_arr) = levels.split_at_mut(levels.len() - 1);
    let cur = &mut cur_arr[0];

    while cur.read_lines > 0 {
        // roll line0 <- line1
        std::mem::swap(&mut cur.line0, &mut cur.line1);

        // recursively get the LL line from the smaller level
        if !smaller.is_empty() {
            let current_line = cur.current_line;
            let rsx = cur.reflect_start_x;
            let lsx = cur.level_size_x;
            let mut ll_refs: Vec<&mut [f32]> = cur
                .line1
                .chunks_mut(cur.stride)
                .enumerate()
                .filter(|(i, _)| i % MAX_SIDEBAND == 0)
                .map(|(_, c)| &mut c[rsx..rsx + lsx])
                .collect();
            pull_line(smaller, ecw, current_line, &mut ll_refs)?;
        }

        let full_w = cur.level_size_x + cur.reflect_start_x + cur.reflect_end_x;
        if cur.current_line >= cur.y_size {
            // Y reflection past the last line: line1 = line0
            for c in 0..cur.bands * MAX_SIDEBAND {
                let base = c * cur.stride;
                cur.line1[base..base + full_w]
                    .copy_from_slice(&cur.line0[base..base + full_w]);
            }
        } else {
            cur.read_sideband_line(ecw)?;
            // reflect line1 into line0 at the very first line
            if cur.reflect_start_y == 1 && cur.current_line == 0 {
                for c in 0..cur.bands * MAX_SIDEBAND {
                    let base = c * cur.stride;
                    let (l0, l1) = (&mut cur.line0, &cur.line1);
                    l0[base..base + full_w].copy_from_slice(&l1[base..base + full_w]);
                }
            }
        }

        // X reflection on line1
        if cur.reflect_start_x == 1 {
            for c in 0..cur.bands * MAX_SIDEBAND {
                let base = c * cur.stride;
                cur.line1[base] = cur.line1[base + 1];
            }
        }
        if cur.reflect_end_x == 1 {
            let idx = cur.level_size_x + cur.reflect_start_x;
            for c in 0..cur.bands * MAX_SIDEBAND {
                let base = c * cur.stride;
                cur.line1[base + idx] = cur.line1[base + idx - 1];
            }
        }

        cur.read_lines -= 1;
        cur.current_line += 1;
    }

    // Synthesize the output line from line0/line1.
    for (band, o) in out.iter_mut().enumerate() {
        let ll0 = &cur.line0[cur.chunk(band, 0)];
        let lh0 = &cur.line0[cur.chunk(band, 1)];
        let hl0 = &cur.line0[cur.chunk(band, 2)];
        let hh0 = &cur.line0[cur.chunk(band, 3)];
        let ll1 = &cur.line1[cur.chunk(band, 0)];
        let lh1 = &cur.line1[cur.chunk(band, 1)];
        let hl1 = &cur.line1[cur.chunk(band, 2)];
        let hh1 = &cur.line1[cur.chunk(band, 3)];
        let o: &mut [f32] = o;
        // Even/odd x phases are computed in separate branch-free passes so the
        // loops can vectorize. Output sample i has parity
        // (output_level_start_x + i) & 1; the rolling input index b advances
        // after each even-parity sample, so a normalized even-start sequence
        // is even(0), odd(1), even(1), odd(2), ... An odd-start sequence is
        // the same after a leading odd(0) sample.
        let start_odd = cur.output_level_start_x & 1 == 1;
        if y_line & 1 == 0 {
            let even = |b: usize| {
                ll1[b + 1]
                    - (lh0[b + 1] + lh1[b + 1] + hl1[b] + hl1[b + 1]) * 0.5
                    + (hh0[b] + hh0[b + 1] + hh1[b] + hh1[b + 1]) * 0.25
            };
            let odd = |b: usize| {
                ((ll1[b] + ll1[b + 1]) - (hh0[b] + hh1[b])) * 0.5
                    - (lh0[b] + lh0[b + 1] + lh1[b] + lh1[b + 1]) * 0.25
                    + hl1[b]
            };
            let rest = if start_odd {
                let (first, rest) = o.split_first_mut().unwrap();
                *first = odd(0);
                rest
            } else {
                o
            };
            let n_pairs = rest.len() / 2;
            let mut pairs = rest.chunks_exact_mut(2);
            for (k, pair) in pairs.by_ref().enumerate() {
                pair[0] = even(k);
                pair[1] = odd(k + 1);
            }
            if let [last] = pairs.into_remainder() {
                *last = even(n_pairs);
            }
        } else {
            let even = |b: usize| {
                ((ll0[b + 1] + ll1[b + 1]) - (hh0[b] + hh0[b + 1])) * 0.5
                    + lh0[b + 1]
                    - (hl0[b] + hl0[b + 1] + hl1[b] + hl1[b + 1]) * 0.25
            };
            let odd = |b: usize| {
                (ll0[b] + ll0[b + 1] + ll1[b] + ll1[b + 1]) * 0.25
                    + (lh0[b] + lh0[b + 1] + hl0[b] + hl1[b]) * 0.5
                    + hh0[b]
            };
            let rest = if start_odd {
                let (first, rest) = o.split_first_mut().unwrap();
                *first = odd(0);
                rest
            } else {
                o
            };
            let n_pairs = rest.len() / 2;
            let mut pairs = rest.chunks_exact_mut(2);
            for (k, pair) in pairs.by_ref().enumerate() {
                pair[0] = even(k);
                pair[1] = odd(k + 1);
            }
            if let [last] = pairs.into_remainder() {
                *last = even(n_pairs);
            }
        }
    }

    // After an even output line the next line needs one more input line.
    if y_line & 1 == 0 {
        cur.read_lines = 1;
    }
    Ok(())
}

impl Region {
    /// Start reading output rows [first_row, first_row + n_rows) of a
    /// `number_x` x `number_y` sampling of the file rectangle
    /// [start_x..=end_x] x [start_y..=end_y]. Level selection and sampling
    /// are computed from the full grid, and the entropy/DWT state is primed
    /// at the strip's first line, so the produced scanlines are identical to
    /// the corresponding rows of a full decode (first_row 0, n_rows
    /// number_y). This is what makes strip-parallel decoding possible.
    #[allow(clippy::too_many_arguments)]
    pub fn start_rows(
        ecw: &EcwFile,
        start_x: u32,
        start_y: u32,
        end_x: u32,
        end_y: u32,
        number_x: u32,
        number_y: u32,
        first_row: u32,
        n_rows: u32,
    ) -> Result<Region, Error> {
        let h = &ecw.header;
        let x_size = h.x_size;
        let y_size = h.y_size;
        if start_x > end_x
            || start_y > end_y
            || end_x >= x_size
            || end_y >= y_size
            || number_x > 1 + end_x - start_x
            || number_y > 1 + end_y - start_y
            || number_x == 0
            || number_y == 0
            || n_rows == 0
            || first_row as u64 + n_rows as u64 > number_y as u64
        {
            return Err("invalid region".into());
        }
        let bands = h.nr_bands;

        // Find the largest level needed for the requested detail.
        let mut out_start_x = start_x;
        let mut out_end_x = end_x;
        let mut out_start_y = start_y;
        let mut out_end_y = end_y;
        let mut out_size_x = 1 + out_end_x - out_start_x;
        let mut out_size_y = 1 + out_end_y - out_start_y;
        let mut lvl = h.num_levels - 1;
        while out_size_y > number_y * 2 && out_size_x > number_x * 2 {
            if lvl == 0 {
                break;
            }
            out_start_x /= 2;
            out_start_y /= 2;
            out_end_x /= 2;
            out_end_y /= 2;
            out_size_x = 1 + out_end_x - out_start_x;
            out_size_y = 1 + out_end_y - out_start_y;
            lvl -= 1;
        }
        if number_x == 1 {
            out_start_x += (out_end_x - out_start_x) / 2;
            out_end_x = out_start_x;
            out_size_x = 1;
        }
        if number_y == 1 {
            out_start_y += (out_end_y - out_start_y) / 2;
            out_end_y = out_start_y;
            out_size_y = 1;
        }

        let largest = lvl;
        let out_w = out_size_x as usize;

        // Driver state for the strip: same global sampling as a full decode
        // (start_line/increment from the full grid), fast-forwarded to
        // first_row. The level machinery below is primed at the level-space
        // line of that row.
        let increment_y = out_size_y as f64 / number_y as f64;
        let start_line = out_start_y as f64;
        let current_line = start_line + increment_y * first_row as f64;
        let strip_start_y = current_line as u32;

        let mut region = Region {
            number_x,
            format: h.compress_format,
            bands,
            levels: Vec::with_capacity(largest + 1),
            start_line,
            current_line,
            increment_x: out_size_x as f64 / number_x as f64,
            increment_y,
            read_line: 1,
            counter: first_row as u64,
            out_w,
            ll: vec![0.0; bands * out_w],
        };

        // Set up each level from `largest` down to 0.
        let mut levels_rev: Vec<RegionLevel> = Vec::with_capacity(largest + 1);
        let mut o_start_x = out_start_x;
        let mut o_end_x = out_end_x;
        let mut o_start_y = strip_start_y;
        let mut o_end_y = out_end_y;
        let mut o_size_x = out_size_x;

        for level in (0..=largest).rev() {
            let li = &ecw.levels[level];
            let (larger_x, larger_y) = if level + 1 == h.num_levels {
                (x_size, y_size)
            } else {
                (ecw.levels[level + 1].x_size, ecw.levels[level + 1].y_size)
            };

            let (level_start_x, reflect_start_x) = if o_start_x != 0 {
                ((o_start_x - 1) / 2, 0usize)
            } else {
                (0, 1)
            };
            let (level_end_x, reflect_end_x) = if o_end_x < larger_x - 1 {
                (if o_end_x != 0 { (o_end_x - 1) / 2 + 1 } else { 0 }, 0usize)
            } else {
                (li.x_size - 1, 1)
            };
            let (level_start_y, reflect_start_y) = if o_start_y != 0 {
                ((o_start_y - 1) / 2, 0usize)
            } else {
                (0, 1)
            };
            let (level_end_y, reflect_end_y) = if o_end_y < larger_y - 1 {
                (if o_end_y != 0 { (o_end_y - 1) / 2 + 1 } else { 0 }, 0usize)
            } else {
                (li.y_size - 1, 1)
            };
            let level_size_x = (1 + level_end_x - level_start_x) as usize;
            let level_size_y = 1 + level_end_y - level_start_y;
            if li.x_size <= level_end_x || li.y_size <= level_end_y {
                return Err(format!("level {level} smaller than expected"));
            }

            let start_x_block = level_start_x / h.x_block_size;
            let last_x_block = level_end_x / h.x_block_size;
            let x_block_count = 1 + last_x_block - start_x_block;
            let first_block_skip = (level_start_x - start_x_block * h.x_block_size) as usize;
            let last_pixel =
                ((last_x_block + 1) * h.x_block_size - 1).min(li.x_size - 1);
            let last_block_skip = (last_pixel - level_end_x) as usize;

            let mut block_infos = Vec::with_capacity(x_block_count as usize);
            for k in 0..x_block_count {
                let bx = start_x_block + k;
                let line_length = if bx != li.nr_x_blocks - 1 {
                    h.x_block_size as usize
                } else {
                    (li.x_size - bx * h.x_block_size) as usize
                };
                let pre_skip = if k == 0 { first_block_skip } else { 0 };
                let post_skip = if k == x_block_count - 1 {
                    last_block_skip
                } else {
                    0
                };
                block_infos.push(XBlockInfo {
                    line_length,
                    pre_skip,
                    post_skip,
                    valid: line_length - pre_skip - post_skip,
                });
            }

            let stride = level_size_x + 2;
            levels_rev.push(RegionLevel {
                level,
                x_size: li.x_size,
                y_size: li.y_size,
                x_block_size: h.x_block_size,
                y_block_size: h.y_block_size,
                bin_sizes: li
                    .bin_sizes
                    .iter()
                    .map(|&b| b as f32 / h.scale_factor)
                    .collect(),
                first_sideband: if level == 0 { 0 } else { 1 },
                bands,
                reflect_start_x,
                reflect_end_x,
                reflect_start_y,
                reflect_end_y,
                level_start_x,
                level_end_x,
                level_size_x,
                level_start_y,
                level_end_y,
                level_size_y,
                output_level_start_x: o_start_x,
                output_level_size_x: o_size_x as usize,
                start_x_block,
                x_block_count,
                block_infos,
                start_line: level_start_y,
                current_line: level_start_y,
                read_lines: 2 - reflect_start_y as u32,
                next_input_block_y_line: 0,
                have_blocks: false,
                blocks: Vec::new(),
                stride,
                line0: vec![0.0; bands * MAX_SIDEBAND * stride],
                line1: vec![0.0; bands * MAX_SIDEBAND * stride],
            });

            // outputs for the next smaller level
            o_start_x = level_start_x;
            o_end_x = level_end_x;
            o_start_y = level_start_y;
            o_end_y = level_end_y;
            o_size_x = level_size_x as u32;
        }
        levels_rev.reverse(); // now index == level
        region.levels = levels_rev;
        Ok(region)
    }

    /// Number of interleaved output channels per pixel (matches
    /// `EcwHeader::output_channels`).
    fn channels(&self) -> usize {
        match self.format {
            CompressFormat::Grey => 1,
            CompressFormat::Yuv => 3,
            _ => self.bands.min(3),
        }
    }

    /// Read the next scanline as interleaved u8 samples, advancing one output
    /// row per call. Thread-safe with respect to `ecw`: block reads use
    /// positioned I/O, so multiple Regions can decode strips of the same file
    /// concurrently. `out` receives `number_x * channels()` bytes.
    pub fn read_scanline(&mut self, ecw: &EcwFile, out: &mut [u8]) -> Result<(), Error> {
        let n_line_y = self.current_line as u32;

        while self.read_line > 0 {
            let y = (n_line_y - self.read_line) + 1;
            let mut refs: Vec<&mut [f32]> = self.ll.chunks_mut(self.out_w).collect();
            pull_line(&mut self.levels, ecw, y, &mut refs)?;
            self.read_line -= 1;
        }

        let nx = self.number_x as usize;
        let inc_x = ((self.increment_x * 65536.0) as u64) << 16;
        let ch = self.channels();
        debug_assert!(out.len() >= nx * ch);

        #[inline]
        fn clamp_u8(v: f32) -> u8 {
            let r = v.round_ties_even() as i32;
            r.clamp(0, 255) as u8
        }

        // At scale 1 the sample index is the identity; use contiguous loops
        // the compiler can vectorize instead of the fixed-point gather.
        let contiguous = inc_x == 1u64 << 32;
        match self.format {
            CompressFormat::Yuv => {
                let (py, rest) = self.ll.split_at(self.out_w);
                let (pu, pv) = rest.split_at(self.out_w);
                if contiguous {
                    let it = py[..nx].iter().zip(&pu[..nx]).zip(&pv[..nx]);
                    for (px, ((&y, &u), &v)) in out.chunks_exact_mut(3).zip(it) {
                        px[0] = clamp_u8(y + 1.402f32 * v);
                        px[1] = clamp_u8(y + (-0.34414f32) * u + (-0.71414f32) * v);
                        px[2] = clamp_u8(y + 1.772f32 * u);
                    }
                } else {
                    let mut x_off = 0u64;
                    for i in 0..nx {
                        let xi = (x_off >> 32) as usize;
                        let y = py[xi];
                        let u = pu[xi];
                        let v = pv[xi];
                        out[i * 3] = clamp_u8(y + 1.402f32 * v);
                        out[i * 3 + 1] = clamp_u8(y + (-0.34414f32) * u + (-0.71414f32) * v);
                        out[i * 3 + 2] = clamp_u8(y + 1.772f32 * u);
                        x_off += inc_x;
                    }
                }
            }
            CompressFormat::Grey => {
                let p = &self.ll[..self.out_w];
                if contiguous {
                    for (o, &v) in out[..nx].iter_mut().zip(&p[..nx]) {
                        *o = clamp_u8(v);
                    }
                } else {
                    let mut x_off = 0u64;
                    for o in out[..nx].iter_mut() {
                        *o = clamp_u8(p[(x_off >> 32) as usize]);
                        x_off += inc_x;
                    }
                }
            }
            _ => {
                // MULTI / RGB: independent bands, no color transform
                for b in 0..ch {
                    let p = &self.ll[b * self.out_w..(b + 1) * self.out_w];
                    if contiguous {
                        for (px, &v) in out.chunks_exact_mut(ch).zip(&p[..nx]) {
                            px[b] = clamp_u8(v);
                        }
                    } else {
                        let mut x_off = 0u64;
                        for i in 0..nx {
                            out[i * ch + b] = clamp_u8(p[(x_off >> 32) as usize]);
                            x_off += inc_x;
                        }
                    }
                }
            }
        }

        // Work out whether the next output line needs new input lines.
        self.counter += 1;
        let y_next = self.start_line + self.increment_y * self.counter as f64;
        if (y_next as u32) <= (self.current_line as u32) {
            self.read_line = 0;
        } else {
            self.read_line = y_next as u32 - self.current_line as u32;
        }
        self.current_line = y_next;
        Ok(())
    }
}
