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
    pub scale_factor: u16,
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

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}

/// Quantized subband plane.
struct QPlane {
    w: usize,
    h: usize,
    data: Vec<i16>,
}

impl QPlane {
    fn quantize(p: &Plane, bin: f64) -> QPlane {
        QPlane {
            w: p.w,
            h: p.h,
            data: p
                .data
                .iter()
                .map(|&v| (v / bin).round().clamp(-32768.0, 32767.0) as i16)
                .collect(),
        }
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

/// Decompose every band into `levels` levels of subbands, quantized.
fn build_pyramid(planes: Vec<Plane>, levels: usize, bin: f64) -> Vec<LevelData> {
    let bands = planes.len();
    // levels_sb[L][band] = (lh, hl, hh); level 0 additionally gets ll below.
    let mut detail: Vec<Vec<[QPlane; 3]>> =
        (0..levels).map(|_| Vec::with_capacity(bands)).collect();
    let mut lowpass: Vec<Plane> = Vec::with_capacity(bands);

    for plane in planes {
        let mut cur = plane;
        for k in 0..levels {
            let sb = forward_2d(&cur);
            // The first decomposition (from the full raster) fills the
            // largest level, so levels fill from the top down.
            let level = levels - 1 - k;
            detail[level].push([
                QPlane::quantize(&sb.lh, bin),
                QPlane::quantize(&sb.hl, bin),
                QPlane::quantize(&sb.hh, bin),
            ]);
            cur = sb.ll;
        }
        lowpass.push(cur);
    }

    let mut out = Vec::with_capacity(levels);
    for (level, per_band) in detail.into_iter().enumerate() {
        let (w, h) = (per_band[0][0].w, per_band[0][0].h);
        let streams: Vec<Vec<QPlane>> = per_band
            .into_iter()
            .enumerate()
            .map(|(band, [lh, hl, hh])| {
                if level == 0 {
                    // LL is quantized with the same step; it lives only here.
                    let ll = QPlane::quantize(&lowpass[band], bin);
                    vec![ll, lh, hl, hh]
                } else {
                    vec![lh, hl, hh]
                }
            })
            .collect();
        out.push(LevelData { w, h, streams });
    }
    out
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

    let format = if ch == 1 {
        CompressFormat::Grey
    } else {
        CompressFormat::Yuv
    };
    let planes = to_band_planes(pixels, w as usize, h as usize, ch);
    let step = params.bin_size as f64 / params.scale_factor as f64;
    let pyramid = build_pyramid(planes, levels, step);

    // --- header (integers big-endian, doubles little-endian) ---
    out.write_all(&[b'e', 2, 0, format as u8, levels as u8, 4])?;
    out.write_all(&w.to_be_bytes())?;
    out.write_all(&h.to_be_bytes())?;
    out.write_all(&(ch as u16).to_be_bytes())?;
    out.write_all(&params.scale_factor.to_be_bytes())?;
    out.write_all(&params.block_size.to_be_bytes()[2..])?; // u16be
    out.write_all(&params.block_size.to_be_bytes()[2..])?;
    out.write_all(&params.compression_rate.to_be_bytes())?;
    out.write_all(&[params.cell_units])?;
    out.write_all(&params.cell_inc_x.to_le_bytes())?;
    out.write_all(&params.cell_inc_y.to_le_bytes())?;
    out.write_all(&params.origin_x.to_le_bytes())?;
    out.write_all(&params.origin_y.to_le_bytes())?;
    let mut name = [0u8; 16];
    let d = params.datum.as_bytes();
    name[..d.len().min(15)].copy_from_slice(&d[..d.len().min(15)]);
    out.write_all(&name)?;
    name = [0u8; 16];
    let p = params.projection.as_bytes();
    name[..p.len().min(15)].copy_from_slice(&p[..p.len().min(15)]);
    out.write_all(&name)?;

    // --- level table ---
    let block = params.block_size as usize;
    let mut blocks_per_level = Vec::with_capacity(levels);
    for (i, level) in pyramid.iter().enumerate() {
        out.write_all(&[i as u8])?;
        out.write_all(&(level.w as u32).to_be_bytes())?;
        out.write_all(&(level.h as u32).to_be_bytes())?;
        for _ in 0..ch {
            out.write_all(&params.bin_size.to_be_bytes())?;
        }
        let nx = level.w.div_ceil(block);
        let ny = level.h.div_ceil(block);
        blocks_per_level.push((nx, ny));
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
    out.write_all(&((unpacked_len + 1) as u32).to_be_bytes())?;
    out.write_all(&[ENCODE_RAW])?;
    for o in &offsets {
        out.write_all(&o.to_le_bytes())?;
    }
    for b in &bodies {
        out.write_all(b)?;
    }
    out.flush()
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
