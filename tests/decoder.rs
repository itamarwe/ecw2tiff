//! Decoder regression tests against a checked-in ECW fixture.
//!
//! `data/alt16.ecw` is a 16x16 single-band ECW v2 file with a 3-level
//! pyramid whose image is a 1-pixel alternating pattern (20 / 235). That
//! pattern is a worst case for edge handling: it puts maximum detail energy
//! in every subband, right up against the image borders.
//!
//! The expected output is the pattern itself — defined by the image, not by
//! whatever the decoder currently produces — so these assertions are ground
//! truth rather than a snapshot of current behaviour. (The file was produced
//! by this project's encoder from that pattern; it decodes to it exactly.)

use ecw2tiff::codec::{EcwImage, Window};
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
    let ecw = EcwImage::open(&tmp.0).expect("fixture should open");
    let (w, h) = (ecw.info().width, ecw.info().height);
    assert_eq!((w as usize, h as usize), (SIZE, SIZE));
    let ch = ecw.info().channels();
    assert_eq!(ch, 1);
    let window = Window {
        x0: 0,
        y0: 0,
        x1: w - 1,
        y1: h - 1,
        out_width: w,
        out_height: h,
    };
    let mut rows = ecw.rows(window, 0, h).expect("row reader");
    let mut out = vec![0u8; SIZE * SIZE];
    for y in 0..SIZE {
        rows.read_row(&mut out[y * SIZE..(y + 1) * SIZE])
            .unwrap_or_else(|e| panic!("decode failed at row {y}: {e}"));
    }
    out
}

/// The top-left pixel sits where edge replication matters most at every
/// stage of the pyramid.
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
