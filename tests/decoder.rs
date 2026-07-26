//! Decoder regression tests against a checked-in ECW fixture.
//!
//! `data/alt16.ecw` is a 16x16 single-band ECW v2 file with a 3-level
//! pyramid whose image is a 1-pixel alternating pattern (20 / 235). That
//! pattern is the worst case for these bugs: it puts maximum detail energy
//! in every subband, right up against the top-left edge.
//!
//! The expected output is the pattern itself — defined by the image, not by
//! whatever the decoder currently produces — so these assertions are ground
//! truth rather than a snapshot of current behaviour. (The file was produced
//! by this project's encoder from that pattern; it decodes to it exactly.)

use ecw2tiff::header::EcwFile;
use ecw2tiff::idwt::Region;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const FIXTURE: &[u8] = include_bytes!("data/alt16.ecw");
const SIZE: usize = 16;

/// The image the fixture encodes: alternating 20 / 235, starting at 20.
fn expected() -> Vec<u8> {
    (0..SIZE * SIZE)
        .map(|i| {
            if ((i % SIZE) + (i / SIZE)).is_multiple_of(2) {
                20u8
            } else {
                235
            }
        })
        .collect()
}

struct TempEcw(PathBuf);

impl TempEcw {
    fn new() -> TempEcw {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "ecw2tiff-dec-{}-{}.ecw",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let mut f = std::fs::File::create(&p).expect("create temp ecw");
        f.write_all(FIXTURE).expect("write temp ecw");
        TempEcw(p)
    }
}

impl Drop for TempEcw {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn decode_fixture() -> Vec<u8> {
    let tmp = TempEcw::new();
    let ecw = EcwFile::open(&tmp.0).expect("fixture should open");
    let (w, h) = (ecw.header.x_size, ecw.header.y_size);
    assert_eq!((w as usize, h as usize), (SIZE, SIZE));
    let ch = ecw.header.output_channels();
    assert_eq!(ch, 1);
    let mut region =
        Region::start_rows(&ecw, 0, 0, w - 1, h - 1, w, h, 0, h).expect("region start");
    let mut out = vec![0u8; SIZE * SIZE];
    for y in 0..SIZE {
        region
            .read_scanline(&ecw, &mut out[y * SIZE..(y + 1) * SIZE])
            .unwrap_or_else(|e| panic!("decode failed at row {y}: {e}"));
    }
    out
}

/// Regression: `read_scanline` computed `(n_line_y - read_line) + 1`, which
/// underflows on the very first scanline of any decode (`0 - 1`). Release
/// builds wrapped to the correct value, so this only ever panicked in debug
/// builds — and there were no decode tests to notice. Simply reaching the end
/// of this test in a debug build covers it.
#[test]
fn first_scanline_does_not_underflow() {
    let got = decode_fixture();
    assert_eq!(got.len(), SIZE * SIZE);
}

/// Regression: `pull_line` copied `line1` into `line0` for the first output
/// line *before* applying the x-edge replication, so `line0`'s edge sample
/// stayed zero. The even-row synthesis reads `hl0`/`hh0` at that index, so
/// the top-left corner decoded wrong wherever edge detail was non-zero, and
/// the error compounded through the pyramid. Before the fix this pixel
/// decoded as 47 instead of 20.
#[test]
fn top_left_corner_with_edge_detail() {
    let got = decode_fixture();
    assert_eq!(
        got[0], 20,
        "top-left pixel decoded as {} (expected 20)",
        got[0]
    );
}

/// The whole fixture must decode exactly to the alternating pattern.
#[test]
fn fixture_decodes_exactly() {
    let got = decode_fixture();
    let want = expected();
    if got != want {
        let bad = got
            .iter()
            .zip(&want)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| format!("({},{}): {a} != {b}", i % SIZE, i / SIZE))
            .take(8)
            .collect::<Vec<_>>();
        panic!(
            "decoded image differs at {} pixels: {bad:?}",
            got.iter().zip(&want).filter(|(a, b)| a != b).count()
        );
    }
}
