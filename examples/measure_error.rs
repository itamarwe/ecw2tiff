//! Round-trip accuracy of the ECW encoder, measured against the decoder.
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
    let cases: Vec<(&str, u32, u32, usize, Vec<u8>)> = vec![
        (
            "grey gradient 128x96",
            128,
            96,
            1,
            (0..128u32 * 96)
                .map(|i| ((i % 128 * 2 + i / 128) % 256) as u8)
                .collect(),
        ),
        (
            "checkerboard 64x64",
            64,
            64,
            1,
            (0..64u32 * 64)
                .map(|i| {
                    if ((i % 64) / 8 + (i / 64) / 8) % 2 == 0 {
                        20
                    } else {
                        235
                    }
                })
                .collect(),
        ),
        (
            "1px alternating 64x64",
            64,
            64,
            1,
            (0..64u32 * 64)
                .map(|i| {
                    if ((i % 64) + (i / 64)) % 2 == 0 {
                        20
                    } else {
                        235
                    }
                })
                .collect(),
        ),
        (
            "smooth 320x256",
            320,
            256,
            1,
            (0..320u32 * 256)
                .map(|i| {
                    let (x, y) = ((i % 320) as f64, (i / 320) as f64);
                    (128.0 + 90.0 * ((x / 20.0).sin() + (y / 15.0).cos()) / 2.0) as u8
                })
                .collect(),
        ),
        ("rgb 128x128", 128, 128, 3, {
            let mut v = Vec::new();
            for y in 0..128 {
                for x in 0..128 {
                    let (fx, fy) = (x as f64 / 128.0, y as f64 / 128.0);
                    v.push((120.0 + 100.0 * (fx * 6.0).sin()) as u8);
                    v.push((130.0 + 90.0 * (fy * 5.0).cos()) as u8);
                    v.push((140.0 + 80.0 * ((fx + fy) * 4.0).sin()) as u8);
                }
            }
            v
        }),
    ];
    println!(
        "{:<24} {:>6} {:>8} {:>9} {:>10} {:>7}",
        "case", "scale", "max err", "mean err", "bytes", "vs raw"
    );
    for (name, w, h, ch, px) in &cases {
        for sf in [1u16, 16] {
            let p = EncodeParams {
                scale_factor: sf,
                ..EncodeParams::default()
            };
            let mut buf = Vec::new();
            encode(&mut buf, px, *w, *h, *ch, &p).unwrap();
            let tmp = std::env::temp_dir().join(format!("m-{}-{}.ecw", std::process::id(), sf));
            std::fs::write(&tmp, &buf).unwrap();
            let (got, _) = decode_all(&tmp);
            let _ = std::fs::remove_file(&tmp);
            let mut max = 0u8;
            let mut tot = 0u64;
            for (a, b) in px.iter().zip(&got) {
                let d = a.abs_diff(*b);
                max = max.max(d);
                tot += d as u64;
            }
            println!(
                "{:<24} {:>6} {:>8} {:>9.4} {:>10} {:>6.2}x",
                if sf == 1 { *name } else { "" },
                sf,
                max,
                tot as f64 / px.len() as f64,
                buf.len(),
                buf.len() as f64 / px.len() as f64
            );
        }
    }
}
