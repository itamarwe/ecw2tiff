//! Round-trip accuracy and compression of the ECW encoder, measured against
//! the real decoder. Run with: cargo run --release --example encode_quality
use ecw2tiff::encode::{encode, EncodeParams};
use ecw2tiff::header::EcwFile;
use ecw2tiff::idwt::Region;

fn decode_all(path: &std::path::Path) -> (Vec<u8>, usize) {
    let ecw = EcwFile::open(path).unwrap();
    let (w, h) = (ecw.header.x_size, ecw.header.y_size);
    let ch = ecw.header.output_channels();
    let mut r = Region::start_rows(&ecw, 0, 0, w - 1, h - 1, w, h, 0, h).unwrap();
    let row = w as usize * ch;
    let mut out = vec![0u8; row * h as usize];
    for y in 0..h as usize {
        r.read_scanline(&ecw, &mut out[y * row..(y + 1) * row])
            .unwrap();
    }
    (out, ch)
}

fn main() {
    let (w, h) = (256u32, 256u32);
    let px: Vec<u8> = (0..w * h * 3)
        .map(|i| {
            let p = i / 3;
            let (x, y) = ((p % w) as f64, (p / w) as f64);
            let base = 128.0 + 80.0 * ((x / 17.0).sin() + (y / 21.0).cos()) / 2.0;
            (base + (i % 3) as f64 * 12.0) as u8
        })
        .collect();
    let raw = px.len() as f64;

    println!(
        "{:<12} {:>10} {:>10} {:>9} {:>10}",
        "mode", "bytes", "ratio", "max err", "mean err"
    );
    for target in [None, Some(5u16), Some(20), Some(50), Some(100)] {
        let params = EncodeParams {
            target_ratio: target,
            ..EncodeParams::default()
        };
        let mut buf = Vec::new();
        encode(&mut buf, &px, w, h, 3, &params).unwrap();
        let tmp = std::env::temp_dir().join(format!("eq-{}-{:?}.ecw", std::process::id(), target));
        std::fs::write(&tmp, &buf).unwrap();
        let (got, _) = decode_all(&tmp);
        let _ = std::fs::remove_file(&tmp);
        let d: Vec<u32> = px
            .iter()
            .zip(&got)
            .map(|(a, b)| a.abs_diff(*b) as u32)
            .collect();
        let label = match target {
            None => "lossless".to_string(),
            Some(r) => format!("ratio {r}"),
        };
        println!(
            "{:<12} {:>10} {:>9.1}x {:>9} {:>10.3}",
            label,
            buf.len(),
            raw / buf.len() as f64,
            d.iter().max().unwrap(),
            d.iter().sum::<u32>() as f64 / d.len() as f64
        );
    }
}
