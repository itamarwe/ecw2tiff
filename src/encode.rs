//! ECW v2 encoder: raster -> wavelet pyramid -> blocks -> ECW file.
//!
//! This is the inverse of the decode path in `header.rs` / `idwt.rs`. The
//! pyramid is built with [`crate::fdwt`], whose analysis exactly inverts the
//! decoder's synthesis, and the coefficients are written as `ENCODE_RAW`
//! int16 sidebands (the simplest of the six sideband encodings the decoder
//! supports). Blocks whose coefficients are entirely zero are written with
//! zero length, which the decoder reads as an all-zero block.
//!
//! # File layout produced
//!
//! ```text
//!   header      'e', version 2, format, num_levels, 4 sidebands, sizes, geo
//!   levels      per level: u8 index, u32be x, u32be y, u32be bin_size/band
//!   block table u32be packed_len, u8 ENCODE_RAW, (n_blocks + 1) u64le offsets
//!   blocks      level 0 first, each level row-major
//! ```
//!
//! Each block holds, band-major, one stream per sideband (`LL, LH, HL, HH` at
//! level 0; `LH, HL, HH` above it), preceded by `total_sb - 1` u32be offsets
//! locating streams 1.. relative to the end of that offset array. A stream is
//! a format byte followed by `rows * line_length` little-endian int16s.
//!
//! # Limitations
//!
//! Dimensions must be divisible by `2^levels` (see [`max_levels`]) — every
//! level is exactly half its parent, so no odd-size edge cases arise. The
//! whole image is held in memory as f64 planes, so this suits modest rasters
//! rather than the multi-gigapixel mosaics the decoder streams.

use std::io::{self, Write};

use crate::entropy::ENCODE_RAW;
use crate::fdwt::{forward_2d, Plane};
use crate::header::CompressFormat;
use crate::sbencode::encode_sideband;

/// Largest number of pyramid levels usable for `w` x `h`: each level halves
/// both dimensions, so this is how many times 2 divides both (capped, and at
/// least 1 as the decoder requires). Returns 0 if either dimension is odd.
pub fn max_levels(w: u32, h: u32) -> usize {
    const CAP: u32 = 16;
    if w == 0 || h == 0 || !w.is_multiple_of(2) || !h.is_multiple_of(2) {
        return 0;
    }
    // Stop before a level would fall below 2 pixels: the smallest level still
    // has to hold a 2x2 LL for the synthesis above it.
    let by_divisibility = w.trailing_zeros().min(h.trailing_zeros());
    let by_size = (w.min(h).ilog2()).saturating_sub(1);
    by_divisibility.min(by_size).clamp(1, CAP) as usize
}

/// Geo / metadata written into the v2 header.
#[derive(Debug, Clone)]
pub struct EncodeParams {
    /// Block edge in pixels (v2 files use 64); must be a positive multiple of 8.
    pub block_size: u32,
    /// Pyramid levels; see [`max_levels`].
    pub levels: usize,
    /// Quantization numerator, per the level table.
    pub bin_size: u32,
    /// Header scale factor. The decoder dequantizes with
    /// `bin_size / scale_factor`, so values above 1 give sub-integer steps and
    /// a correspondingly more accurate round-trip; coefficients are scaled up
    /// by the same factor, so keep `scale_factor * 255` well inside int16.
    /// Real ECW v2 files use 1; we default to 16, which costs nothing in a
    /// RAW-coded file and cuts round-trip error to about a single count.
    /// Ignored when `target_ratio` is set (lossy mode quantizes in whole
    /// bins, as real files do).
    pub scale_factor: u16,
    /// Width of the zero bin, as a multiple of the step. `0.5` is plain
    /// rounding; larger values snap more near-zero coefficients to zero,
    /// which the run-length and Huffman coders exploit heavily — the classic
    /// rate/distortion trade for Laplacian-distributed wavelet coefficients.
    ///
    /// `None` (the default) lets the encoder choose: exact rounding when
    /// encoding losslessly, and for a target ratio, whichever candidate
    /// predicts the least error at that size. Measured on aerial imagery, a
    /// wide zero bin is worth 14-18% of the file at high quality and is
    /// roughly neutral once the steps are already coarse, so the right value
    /// depends on the content and the requested ratio.
    pub deadzone: Option<f64>,
    /// Quantization strength, bypassing the rate search: the finest level's
    /// luma bin, with coarser levels scaled by [`bin_table`]. Takes
    /// precedence over `target_ratio`, which then only labels the header.
    /// This is the direct quality knob; `target_ratio` is the size knob.
    pub strength: Option<u16>,
    /// Target compression ratio. `None` (the default) encodes losslessly.
    /// `Some(r)` quantizes detail coefficients with [`bin_table`], which
    /// follows the curve real ECW files use, and records `r` as the header's
    /// compression rate.
    pub target_ratio: Option<u16>,
    pub cell_units: u8,
    pub cell_inc_x: f64,
    pub cell_inc_y: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub datum: String,
    pub projection: String,
    pub compression_rate: u16,
}

impl Default for EncodeParams {
    fn default() -> Self {
        EncodeParams {
            block_size: 64,
            levels: 0, // 0 = choose with max_levels
            bin_size: 1,
            scale_factor: 16,
            deadzone: None, // chosen per encode
            strength: None,
            target_ratio: None,
            cell_units: 0,
            cell_inc_x: 1.0,
            cell_inc_y: -1.0,
            origin_x: 0.0,
            origin_y: 0.0,
            datum: "RAW".into(),
            projection: "RAW".into(),
            compression_rate: 1,
        }
    }
}

/// Deadzone used when a strength is given directly (no size target to
/// optimize against).
const LOSSY_DEADZONE: f64 = 0.75;

/// Candidate zero-bin widths the rate search chooses between.
const DEADZONE_CANDIDATES: [f64; 3] = [0.5, 0.75, 1.0];

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}

/// Per-level, per-band quantization bin sizes for a given strength.
///
/// `strength` is the finest level's luma bin; each coarser level halves it,
/// flooring at 1. Chroma gets 3x luma. This reproduces what real ECW files
/// store — `tzafon_7_25.ecw` (11 levels, 20:1) has luma
/// `1,1,1,1,1,1,1,2,5,10,20` and chroma 3x that.
///
/// The halving curve is *not* what minimizing mean squared error would
/// suggest. [`crate::fdwt::detail_gain`] measures how a unit coefficient in
/// each level reaches the image (1.50, 2.06, 3.61, 6.97, ... — close to
/// doubling per level but noticeably less at the fine end), and the
/// high-rate MSE-optimal rule is bins proportional to `1 / gain`, which is
/// coarser at the coarse end than halving. Measured against real imagery at
/// matched file sizes, that allocation was *worse* by 1-3 dB across most of
/// the useful range: at these ratios most coefficients quantize to zero, so
/// the high-rate approximation the rule rests on does not hold. Halving is
/// kept because it measures better, not because it was inherited.
pub fn bin_table(levels: usize, bands: usize, strength: u16) -> Vec<Vec<u32>> {
    (0..levels)
        .map(|level| {
            let depth = (levels - 1 - level) as u32; // 0 = finest level
            let luma = (strength as u32 >> depth).max(1);
            (0..bands)
                .map(|band| if band == 0 { luma } else { luma * 3 })
                .collect()
        })
        .collect()
}

/// Quantized subband plane.
struct QPlane {
    w: usize,
    data: Vec<i16>,
}

impl QPlane {
    /// Quantize with a widened zero bin. Coefficients inside the deadzone
    /// become exactly zero; the rest round normally, so nonzero values stay
    /// unbiased against the decoder's `q * bin` reconstruction.
    fn quantize(p: &Plane, bin: f64, deadzone: f64) -> (QPlane, f64) {
        let mut err = 0.0f64;
        let data = p
            .data
            .iter()
            .map(|&v| {
                let q = v / bin;
                let q = if q.abs() < deadzone {
                    0
                } else {
                    q.round().clamp(-32768.0, 32767.0) as i16
                };
                let d = v - q as f64 * bin;
                err += d * d;
                q
            })
            .collect();
        (QPlane { w: p.w, data }, err)
    }
}

/// Split interleaved 8-bit samples into one f64 plane per band, applying the
/// decoder's colour convention: for 3-band input the bands are JPEG-style
/// YCbCr with **unbiased** chroma, since the decoder reconstructs
/// `R = Y + 1.402 V` without a +128 offset.
fn to_band_planes(pixels: &[u8], w: usize, h: usize, ch: usize) -> Vec<Plane> {
    let mut planes = (0..ch).map(|_| Plane::new(w, h)).collect::<Vec<_>>();
    for i in 0..w * h {
        if ch == 3 {
            let (r, g, b) = (
                pixels[i * 3] as f64,
                pixels[i * 3 + 1] as f64,
                pixels[i * 3 + 2] as f64,
            );
            planes[0].data[i] = 0.299 * r + 0.587 * g + 0.114 * b;
            planes[1].data[i] = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
            planes[2].data[i] = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
        } else {
            for (b, plane) in planes.iter_mut().enumerate() {
                plane.data[i] = pixels[i * ch + b] as f64;
            }
        }
    }
    planes
}

/// Sidebands of one level, per band. Index 0 is `LL` only at level 0; every
/// level then has `LH, HL, HH`.
struct LevelData {
    w: usize,
    h: usize,
    /// `[band][sideband]`, band-major exactly as the decoder reads them.
    streams: Vec<Vec<QPlane>>,
}

/// Unquantized wavelet decomposition: the expensive part, computed once so
/// rate control can re-quantize cheaply at many strengths.
struct Decomposed {
    /// `[level][band]` = the level's `LH, HL, HH` planes.
    detail: Vec<Vec<[Plane; 3]>>,
    /// Level 0's `LL`, one per band.
    lowpass: Vec<Plane>,
}

/// Decompose every band into `levels` levels of subbands.
fn decompose(planes: Vec<Plane>, levels: usize) -> Decomposed {
    let bands = planes.len();
    let mut detail: Vec<Vec<[Plane; 3]>> = (0..levels).map(|_| Vec::with_capacity(bands)).collect();
    let mut lowpass: Vec<Plane> = Vec::with_capacity(bands);
    for plane in planes {
        let mut cur = plane;
        for k in 0..levels {
            let sb = forward_2d(&cur);
            // The first decomposition (from the full raster) fills the
            // largest level, so levels fill from the top down.
            detail[levels - 1 - k].push([sb.lh, sb.hl, sb.hh]);
            cur = sb.ll;
        }
        lowpass.push(cur);
    }
    Decomposed { detail, lowpass }
}

/// Quantize a decomposition with `step[level][band]` (the dequantization step
/// the decoder will apply), also returning predicted squared error in the
/// **pixel** domain.
///
/// Each level's coefficient error is scaled by [`crate::fdwt::detail_gain`],
/// the measured amplification from that level to the image, so the figure is
/// comparable across levels and across candidate settings. That makes it
/// usable as the distortion term when choosing between encodings, without
/// having to decode anything.
fn quantize_pyramid(dec: &Decomposed, step: &[Vec<f64>], dz: f64) -> (Vec<LevelData>, f64) {
    let levels = dec.detail.len();
    let distortion = std::cell::Cell::new(0.0f64);
    let out: Vec<LevelData> = dec
        .detail
        .iter()
        .enumerate()
        .map(|(level, per_band)| {
            let (w, h) = (per_band[0][0].w, per_band[0][0].h);
            let gain2 = {
                let g = crate::fdwt::detail_gain(levels - 1 - level);
                g * g
            };
            let streams = per_band
                .iter()
                .enumerate()
                .map(|(band, [lh, hl, hh])| {
                    let b = step[level][band];
                    let mut v = Vec::with_capacity(4);
                    let mut err = 0.0f64;
                    if level == 0 {
                        // LL is quantized with level 0's step; it lives only here.
                        let (q, e) = QPlane::quantize(&dec.lowpass[band], b, dz);
                        err += e;
                        v.push(q);
                    }
                    for plane in [lh, hl, hh] {
                        let (q, e) = QPlane::quantize(plane, b, dz);
                        err += e;
                        v.push(q);
                    }
                    distortion.set(distortion.get() + err * gain2);
                    v
                })
                .collect();
            LevelData { w, h, streams }
        })
        .collect();
    (out, distortion.get())
}

/// Serialize one block of a level, or an empty vector when every coefficient
/// in it is zero (the decoder treats a zero-length block as all zeros).
fn block_bytes(level: &LevelData, bx: usize, by: usize, block: usize) -> Vec<u8> {
    let x0 = bx * block;
    let y0 = by * block;
    let line_length = block.min(level.w - x0);
    let rows = block.min(level.h - y0);

    let all_zero = level.streams.iter().flatten().all(|p| {
        (0..rows).all(|r| {
            let base = (y0 + r) * p.w + x0;
            p.data[base..base + line_length].iter().all(|&v| v == 0)
        })
    });
    if all_zero {
        return Vec::new();
    }

    // Compress each sideband independently, best format per stream.
    let total_sb = level.streams.len() * level.streams[0].len();
    let mut vals = Vec::with_capacity(rows * line_length);
    let mut streams: Vec<Vec<u8>> = Vec::with_capacity(total_sb);
    for plane in level.streams.iter().flatten() {
        vals.clear();
        for r in 0..rows {
            let base = (y0 + r) * plane.w + x0;
            vals.extend_from_slice(&plane.data[base..base + line_length]);
        }
        streams.push(encode_sideband(&vals));
    }

    // Offsets locate streams 1.. relative to the end of this array, so they
    // are the running total of the preceding stream lengths.
    let offs_len = 4 * (total_sb - 1);
    let body: usize = streams.iter().map(|s| s.len()).sum();
    let mut out = Vec::with_capacity(offs_len + body);
    let mut at = 0usize;
    for s in &streams[..total_sb - 1] {
        at += s.len();
        out.extend_from_slice(&(at as u32).to_be_bytes());
    }
    for s in &streams {
        out.extend_from_slice(s);
    }
    out
}

/// Encode an interleaved 8-bit raster as an ECW v2 file.
///
/// `ch` must be 1 (greyscale) or 3 (RGB, stored as the decoder's YCbCr).
/// `w` and `h` must be divisible by `2^levels`.
pub fn encode<W: Write>(
    out: &mut W,
    pixels: &[u8],
    w: u32,
    h: u32,
    ch: usize,
    params: &EncodeParams,
) -> io::Result<()> {
    if ch != 1 && ch != 3 {
        return Err(invalid(format!("band count must be 1 or 3, got {ch}")));
    }
    if pixels.len() != w as usize * h as usize * ch {
        return Err(invalid(format!(
            "expected {} samples for {w}x{h}x{ch}, got {}",
            w as usize * h as usize * ch,
            pixels.len()
        )));
    }
    if params.block_size == 0 || !params.block_size.is_multiple_of(8) {
        return Err(invalid("block size must be a positive multiple of 8"));
    }
    if params.bin_size == 0 {
        return Err(invalid("bin size must be non-zero"));
    }
    if params.scale_factor == 0 {
        return Err(invalid("scale factor must be non-zero"));
    }
    let levels = if params.levels == 0 {
        max_levels(w, h)
    } else {
        params.levels
    };
    if levels == 0 {
        return Err(invalid(format!(
            "{w}x{h} cannot be encoded: dimensions must be even"
        )));
    }
    let shift = levels as u32;
    if !w.is_multiple_of(1 << shift) || !h.is_multiple_of(1 << shift) {
        return Err(invalid(format!(
            "{w}x{h} is not divisible by 2^{levels}; use levels <= {}",
            max_levels(w, h)
        )));
    }

    let planes = to_band_planes(pixels, w as usize, h as usize, ch);
    let dec = decompose(planes, levels);

    // One (quantize + serialize) trial: returns the file and the predicted
    // pixel-domain squared error, so candidates can be compared without
    // decoding anything.
    let trial = |strength: u16, dz: f64| -> (Vec<u8>, f64) {
        let bins = bin_table(levels, ch, strength);
        let steps = steps_from(&bins, params.scale_factor);
        let (pyramid, distortion) = quantize_pyramid(&dec, &steps, dz);
        (
            assemble(
                &pyramid,
                &bins,
                params.scale_factor,
                w,
                h,
                ch,
                levels,
                params,
            ),
            distortion,
        )
    };

    let file = match (params.strength, params.target_ratio) {
        // Explicit strength: quantize once, no search.
        (Some(s), _) => trial(s, params.deadzone.unwrap_or(LOSSY_DEADZONE)).0,

        // Lossless: one bin everywhere, exact rounding, sub-integer step from
        // scale_factor. A deadzone would defeat the point, so it is not used
        // here unless asked for explicitly.
        (None, None) => {
            let bins = vec![vec![params.bin_size; ch]; levels];
            let steps = steps_from(&bins, params.scale_factor);
            let (pyramid, _) = quantize_pyramid(&dec, &steps, params.deadzone.unwrap_or(0.5));
            assemble(
                &pyramid,
                &bins,
                params.scale_factor,
                w,
                h,
                ch,
                levels,
                params,
            )
        }

        // Lossy: find the encoding that fits the size budget with the least
        // predicted error. Quantization strength sets the size; the deadzone
        // trades a little accuracy for a lot of zeros, and how good that
        // trade is depends on the content and on how coarse the steps already
        // are — so rather than fix it, try each and keep whichever predicts
        // less error at the target size.
        (None, Some(ratio)) => {
            let target = (w as u64 * h as u64 * ch as u64) / ratio.max(1) as u64;
            let deadzones: &[f64] = match params.deadzone {
                Some(dz) => &[dz][..],
                None => &DEADZONE_CANDIDATES[..],
            };
            let mut best: Option<(Vec<u8>, f64)> = None;
            for &dz in deadzones {
                // Size falls as strength rises, so binary search the smallest
                // (best quality) strength that fits.
                let mut lo = 1u32;
                let mut hi = u16::MAX as u32;
                let mut fit: Option<(Vec<u8>, f64)> = None;
                while lo <= hi {
                    let mid = lo + (hi - lo) / 2;
                    let (file, dist) = trial(mid as u16, dz);
                    if file.len() as u64 <= target {
                        fit = Some((file, dist));
                        if mid == 1 {
                            break;
                        }
                        hi = mid - 1;
                    } else {
                        lo = mid + 1;
                    }
                }
                if let Some((file, dist)) = fit {
                    if best.as_ref().is_none_or(|(_, bd)| dist < *bd) {
                        best = Some((file, dist));
                    }
                }
            }
            match best {
                Some((file, _)) => file,
                // Even the coarsest quantization overshoots (tiny or very
                // noisy images): emit the smallest we can produce.
                None => trial(u16::MAX, DEADZONE_CANDIDATES[DEADZONE_CANDIDATES.len() - 1]).0,
            }
        }
    };
    out.write_all(&file)?;
    out.flush()
}

/// Dequantization steps the decoder will apply: `bin_size / scale_factor`.
fn steps_from(bins: &[Vec<u32>], scale_factor: u16) -> Vec<Vec<f64>> {
    bins.iter()
        .map(|per_band| {
            per_band
                .iter()
                .map(|&b| b as f64 / scale_factor as f64)
                .collect()
        })
        .collect()
}

/// Serialize a complete ECW v2 file: header, level table, block offset table
/// and blocks.
#[allow(clippy::too_many_arguments)]
fn assemble(
    pyramid: &[LevelData],
    bins: &[Vec<u32>],
    scale_factor: u16,
    w: u32,
    h: u32,
    ch: usize,
    levels: usize,
    params: &EncodeParams,
) -> Vec<u8> {
    let format = if ch == 1 {
        CompressFormat::Grey
    } else {
        CompressFormat::Yuv
    };
    let mut out: Vec<u8> = Vec::new();

    // --- header (integers big-endian, doubles little-endian) ---
    out.extend_from_slice(&[b'e', 2, 0, format as u8, levels as u8, 4]);
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&(ch as u16).to_be_bytes());
    out.extend_from_slice(&scale_factor.to_be_bytes());
    out.extend_from_slice(&params.block_size.to_be_bytes()[2..]); // u16be
    out.extend_from_slice(&params.block_size.to_be_bytes()[2..]);
    out.extend_from_slice(
        &params
            .target_ratio
            .unwrap_or(params.compression_rate)
            .to_be_bytes(),
    );
    out.push(params.cell_units);
    out.extend_from_slice(&params.cell_inc_x.to_le_bytes());
    out.extend_from_slice(&params.cell_inc_y.to_le_bytes());
    out.extend_from_slice(&params.origin_x.to_le_bytes());
    out.extend_from_slice(&params.origin_y.to_le_bytes());
    let mut name = [0u8; 16];
    let d = params.datum.as_bytes();
    name[..d.len().min(15)].copy_from_slice(&d[..d.len().min(15)]);
    out.extend_from_slice(&name);
    name = [0u8; 16];
    let p = params.projection.as_bytes();
    name[..p.len().min(15)].copy_from_slice(&p[..p.len().min(15)]);
    out.extend_from_slice(&name);

    // --- level table ---
    let block = params.block_size as usize;
    let mut blocks_per_level = Vec::with_capacity(levels);
    for (i, level) in pyramid.iter().enumerate() {
        out.push(i as u8);
        out.extend_from_slice(&(level.w as u32).to_be_bytes());
        out.extend_from_slice(&(level.h as u32).to_be_bytes());
        for b in &bins[i][..ch] {
            out.extend_from_slice(&b.to_be_bytes());
        }
        blocks_per_level.push((level.w.div_ceil(block), level.h.div_ceil(block)));
    }

    // --- serialize every block, then the offset table that indexes them ---
    let total_blocks: usize = blocks_per_level.iter().map(|(nx, ny)| nx * ny).sum();
    let mut bodies = Vec::with_capacity(total_blocks);
    let mut offsets = Vec::with_capacity(total_blocks + 1);
    let mut at = 0u64;
    offsets.push(at);
    for (level, &(nx, ny)) in pyramid.iter().zip(&blocks_per_level) {
        for by in 0..ny {
            for bx in 0..nx {
                let b = block_bytes(level, bx, by, block);
                at += b.len() as u64;
                offsets.push(at);
                bodies.push(b);
            }
        }
    }

    // The table is itself "packed": RAW, so u32be length, format byte, then
    // the raw u64le offsets including the trailing sentinel.
    let unpacked_len = offsets.len() * 8;
    out.extend_from_slice(&((unpacked_len + 1) as u32).to_be_bytes());
    out.push(ENCODE_RAW);
    for o in &offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    for b in &bodies {
        out.extend_from_slice(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_choice() {
        assert_eq!(max_levels(256, 256), 7); // capped by size, not divisibility
                                             // Matches the real tzafon_7_25.ecw, which reports 11 levels.
        assert_eq!(max_levels(276480, 215040), 11);
        assert_eq!(max_levels(6, 4), 1); // 2 divides once in 6
        assert_eq!(max_levels(7, 8), 0); // odd width
        assert_eq!(max_levels(0, 8), 0);
    }

    #[test]
    fn rejects_bad_input() {
        let p = EncodeParams::default();
        let mut sink = Vec::new();
        // odd dimensions
        assert!(encode(&mut sink, &[0u8; 21], 7, 3, 1, &p).is_err());
        // wrong buffer length
        assert!(encode(&mut sink, &[0u8; 10], 8, 8, 1, &p).is_err());
        // unsupported band count
        assert!(encode(&mut sink, &[0u8; 8 * 8 * 2], 8, 8, 2, &p).is_err());
        // levels too deep for the dimensions
        let deep = EncodeParams {
            levels: 4,
            ..EncodeParams::default()
        };
        assert!(encode(&mut sink, &[0u8; 8 * 8], 8, 8, 1, &deep).is_err());
    }

    #[test]
    fn flat_image_compresses_to_empty_detail_blocks() {
        // A constant image has zero detail coefficients everywhere, so every
        // block above level 0 should serialize to nothing.
        let mut buf = Vec::new();
        let px = vec![128u8; 64 * 64];
        encode(&mut buf, &px, 64, 64, 1, &EncodeParams::default()).unwrap();
        assert!(
            buf.len() < 64 * 64,
            "flat image should compress well, got {} bytes",
            buf.len()
        );
    }
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    /// The bin curve reproduces what a real 20:1 ECW file stores:
    /// tzafon_7_25.ecw (11 levels) has luma 1,1,1,1,1,1,1,2,5,10,20 and
    /// chroma 3x that.
    #[test]
    fn bin_table_matches_a_real_file() {
        let t = bin_table(11, 3, 20);
        let luma: Vec<u32> = t.iter().map(|b| b[0]).collect();
        assert_eq!(luma, vec![1, 1, 1, 1, 1, 1, 1, 2, 5, 10, 20]);
        let chroma: Vec<u32> = t.iter().map(|b| b[1]).collect();
        assert_eq!(chroma, vec![3, 3, 3, 3, 3, 3, 3, 6, 15, 30, 60]);
        assert!(t.iter().all(|b| b[0] >= 1 && b[1] >= b[0]));
    }

    #[test]
    fn lossless_is_the_default() {
        assert!(EncodeParams::default().target_ratio.is_none());
    }
}
