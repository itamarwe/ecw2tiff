//! Encode -> decode round-trip: every image here is encoded with
//! `ecw2tiff::encode` and then read back through the real decoder
//! (`EcwFile` + `Region`), so these tests exercise the container writer, the
//! forward wavelet transform and the decoder together.

use ecw2tiff::encode::{encode, EncodeParams};
use ecw2tiff::header::EcwFile;
use ecw2tiff::idwt::Region;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Unique temp path (no external crates); removed by `TempEcw::drop`.
struct TempEcw(PathBuf);

impl TempEcw {
    fn write(bytes: &[u8], tag: &str) -> TempEcw {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "ecw2tiff-rt-{}-{}-{}.ecw",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
            tag
        ));
        let mut f = std::fs::File::create(&p).expect("create temp ecw");
        f.write_all(bytes).expect("write temp ecw");
        TempEcw(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempEcw {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Decode a whole ECW file back to interleaved 8-bit samples.
fn decode_all(path: &Path) -> (Vec<u8>, u32, u32, usize) {
    let ecw = EcwFile::open(path).expect("decoder rejected the encoded file");
    let (w, h) = (ecw.header.x_size, ecw.header.y_size);
    let ch = ecw.header.output_channels();
    let mut region =
        Region::start_rows(&ecw, 0, 0, w - 1, h - 1, w, h, 0, h).expect("region start failed");
    let row = w as usize * ch;
    let mut out = vec![0u8; row * h as usize];
    for y in 0..h as usize {
        region
            .read_scanline(&ecw, &mut out[y * row..(y + 1) * row])
            .unwrap_or_else(|e| panic!("decode failed at row {y}: {e}"));
    }
    (out, w, h, ch)
}

/// Max and mean absolute difference between two same-length rasters.
fn diff(a: &[u8], b: &[u8]) -> (u8, f64) {
    assert_eq!(a.len(), b.len());
    let mut max = 0u8;
    let mut total = 0u64;
    for (x, y) in a.iter().zip(b) {
        let d = x.abs_diff(*y);
        max = max.max(d);
        total += d as u64;
    }
    (max, total as f64 / a.len() as f64)
}

fn roundtrip(
    pixels: &[u8],
    w: u32,
    h: u32,
    ch: usize,
    params: &EncodeParams,
    tag: &str,
) -> Vec<u8> {
    let mut buf = Vec::new();
    encode(&mut buf, pixels, w, h, ch, params).expect("encode failed");
    let tmp = TempEcw::write(&buf, tag);
    let (got, gw, gh, gch) = decode_all(tmp.path());
    assert_eq!((gw, gh, gch), (w, h, ch), "{tag}: geometry changed");
    got
}

#[test]
fn grey_gradient_round_trips() {
    let (w, h) = (128u32, 96u32);
    let px: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            ((x * 2 + y) % 256) as u8
        })
        .collect();
    let got = roundtrip(&px, w, h, 1, &EncodeParams::default(), "grey-grad");
    let (max, mean) = diff(&px, &got);
    assert_eq!(max, 0, "grey gradient should be exact (mean {mean:.4})");
}

#[test]
fn flat_grey_is_exact() {
    // A constant image has no detail coefficients, so the round-trip should be
    // exact regardless of quantization.
    let (w, h) = (64u32, 64u32);
    let px = vec![200u8; (w * h) as usize];
    let got = roundtrip(&px, w, h, 1, &EncodeParams::default(), "flat");
    assert_eq!(diff(&px, &got).0, 0, "flat image must round-trip exactly");
}

#[test]
fn rgb_photo_like_round_trips() {
    // Smooth RGB content with structure in all three channels.
    let (w, h) = (128u32, 128u32);
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f64 / w as f64;
            let fy = y as f64 / h as f64;
            px.push((120.0 + 100.0 * (fx * 6.0).sin()) as u8);
            px.push((130.0 + 90.0 * (fy * 5.0).cos()) as u8);
            px.push((140.0 + 80.0 * ((fx + fy) * 4.0).sin()) as u8);
        }
    }
    let got = roundtrip(&px, w, h, 3, &EncodeParams::default(), "rgb");
    let (max, mean) = diff(&px, &got);
    // Exact even through the YCbCr transform at the default scale factor.
    assert_eq!(max, 0, "rgb should be exact (mean {mean:.4})");
}

#[test]
fn sharp_edges_round_trip() {
    // Hard edges are the worst case for a wavelet transform: check the
    // decoder still reproduces them closely.
    let (w, h) = (64u32, 64u32);
    let px: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            if (x / 8 + y / 8) % 2 == 0 {
                20
            } else {
                235
            }
        })
        .collect();
    let got = roundtrip(&px, w, h, 1, &EncodeParams::default(), "edges");
    let (max, mean) = diff(&px, &got);
    assert_eq!(max, 0, "checkerboard should be exact (mean {mean:.4})");
}

#[test]
fn non_square_and_partial_blocks_round_trip() {
    // 96x40 with 64-pixel blocks: every level has partial blocks at the right
    // and bottom edges, exercising the ragged-block paths on both sides.
    let (w, h) = (96u32, 40u32);
    let px: Vec<u8> = (0..w * h).map(|i| ((i * 7) % 251) as u8).collect();
    let params = EncodeParams {
        levels: 3,
        ..EncodeParams::default()
    };
    let got = roundtrip(&px, w, h, 1, &params, "ragged");
    let (max, _) = diff(&px, &got);
    assert_eq!(max, 0, "ragged blocks should be exact");
}

#[test]
fn single_level_and_deep_pyramid_agree() {
    // The pyramid depth changes the container layout completely (block counts,
    // sideband counts per level); both must decode to the same picture.
    let (w, h) = (64u32, 64u32);
    let px: Vec<u8> = (0..w * h)
        .map(|i| (128.0 + 100.0 * ((i % 64) as f64 / 8.0).sin()) as u8)
        .collect();
    for levels in [1usize, 2, 5] {
        let params = EncodeParams {
            levels,
            ..EncodeParams::default()
        };
        let got = roundtrip(&px, w, h, 1, &params, &format!("levels{levels}"));
        let (max, mean) = diff(&px, &got);
        assert_eq!(max, 0, "levels={levels} should be exact (mean {mean:.4})");
    }
}

#[test]
fn geo_metadata_survives() {
    let params = EncodeParams {
        cell_units: 2,
        cell_inc_x: 0.000_003_7,
        cell_inc_y: -0.000_003_7,
        origin_x: 35.081_481,
        origin_y: 33.526_518,
        datum: "WGS84".into(),
        projection: "GEODETIC".into(),
        ..EncodeParams::default()
    };
    let mut buf = Vec::new();
    encode(&mut buf, &vec![100u8; 64 * 64], 64, 64, 1, &params).unwrap();
    let tmp = TempEcw::write(&buf, "geo");
    let ecw = EcwFile::open(tmp.path()).expect("open");
    let hdr = &ecw.header;
    assert_eq!(hdr.version, 2);
    assert_eq!((hdr.x_size, hdr.y_size), (64, 64));
    assert_eq!(hdr.datum, "WGS84");
    assert_eq!(hdr.projection, "GEODETIC");
    assert_eq!(hdr.cell_units, 2);
    assert!((hdr.cell_inc_x - 0.000_003_7).abs() < 1e-12);
    assert!((hdr.origin_x - 35.081_481).abs() < 1e-9);
}

#[test]
fn larger_image_with_many_blocks() {
    // 320x256 at 64-pixel blocks: 5x4 blocks at the top level, several block
    // rows per level, so the block offset table is exercised properly.
    let (w, h) = (320u32, 256u32);
    let px: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            (128.0 + 90.0 * ((x / 20.0).sin() + (y / 15.0).cos()) / 2.0) as u8
        })
        .collect();
    let got = roundtrip(&px, w, h, 1, &EncodeParams::default(), "big");
    let (max, mean) = diff(&px, &got);
    assert_eq!(max, 0, "320x256 should be exact (mean {mean:.4})");
}

/// Regression test for a decoder bug this encoder exposed: `pull_line` copied
/// line1 into line0 for the first output line *before* applying the x-edge
/// replication, leaving line0's edge sample at zero. The even-row synthesis
/// reads hl0/hh0 there, so the top-left corner decoded wrong wherever edge
/// detail was non-zero — and the error grew with pyramid depth because each
/// level's bad corner fed the level above. Smooth imagery hid it (those
/// coefficients are ~0), which is why no real file ever showed it.
#[test]
fn top_left_corner_is_correct_with_edge_detail() {
    let (w, h) = (16u32, 16u32);
    // 1-pixel alternating: maximal detail energy right at the corner.
    let px: Vec<u8> = (0..w * h)
        .map(|i| {
            if ((i % w) + (i / w)) % 2 == 0 {
                20u8
            } else {
                235
            }
        })
        .collect();
    for levels in [1usize, 2, 3] {
        let params = EncodeParams {
            levels,
            ..EncodeParams::default()
        };
        let got = roundtrip(&px, w, h, 1, &params, &format!("corner{levels}"));
        assert_eq!(
            got[0], px[0],
            "levels={levels}: corner pixel decoded as {}, expected {}",
            got[0], px[0]
        );
        assert_eq!(diff(&px, &got).0, 0, "levels={levels}: not exact");
    }
}

/// Lossy mode must actually compress, stay decodable, and track the requested
/// ratio. (Lossless remains the default; every other test above still asserts
/// bit-exactness.)
#[test]
fn lossy_hits_the_requested_ratio() {
    let (w, h) = (256u32, 256u32);
    // Smooth photo-like content, the case rate control is meant for.
    let px: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            (128.0 + 90.0 * ((x / 18.0).sin() + (y / 23.0).cos()) / 2.0) as u8
        })
        .collect();
    let raw = px.len() as f64;

    let mut previous = usize::MAX;
    for ratio in [5u16, 20, 50] {
        let params = EncodeParams {
            target_ratio: Some(ratio),
            ..EncodeParams::default()
        };
        let mut buf = Vec::new();
        encode(&mut buf, &px, w, h, 1, &params).expect("lossy encode");
        let achieved = raw / buf.len() as f64;

        // Never worse than asked for, and within a reasonable band of it.
        assert!(
            achieved >= ratio as f64 * 0.9,
            "ratio {ratio}: only achieved {achieved:.1}x"
        );
        assert!(
            achieved <= ratio as f64 * 3.0,
            "ratio {ratio}: overshot badly at {achieved:.1}x"
        );
        // Coarser targets must not produce larger files.
        assert!(buf.len() < previous, "ratio {ratio} did not shrink output");
        previous = buf.len();

        // And it still decodes to the right geometry, with bounded error.
        let tmp = TempEcw::write(&buf, &format!("lossy{ratio}"));
        let (got, gw, gh, gch) = decode_all(tmp.path());
        assert_eq!((gw, gh, gch), (w, h, 1));
        let (_, mean) = diff(&px, &got);
        assert!(mean < 20.0, "ratio {ratio}: mean error {mean:.2} too high");
    }
}

/// The header records what was requested, so `--info` reports it.
#[test]
fn lossy_records_its_target_in_the_header() {
    let params = EncodeParams {
        target_ratio: Some(25),
        ..EncodeParams::default()
    };
    let mut buf = Vec::new();
    encode(&mut buf, &vec![128u8; 64 * 64], 64, 64, 1, &params).unwrap();
    let tmp = TempEcw::write(&buf, "ratio-hdr");
    let ecw = EcwFile::open(tmp.path()).expect("open");
    assert_eq!(ecw.header.compression_rate, 25);
}
