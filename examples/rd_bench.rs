//! Rate-distortion comparison: PSNR against size for several encoder
//! settings, on a real image.  cargo run --release --example rd_bench -- img.ppm
use ecw2tiff::encode::{encode, EncodeParams};
use ecw2tiff::header::EcwFile;
use ecw2tiff::idwt::Region;

fn read_pnm(bytes: &[u8]) -> (Vec<u8>, u32, u32, usize) {
    let ch = if bytes[1] == b'6' { 3 } else { 1 };
    let mut pos = 2usize;
    let mut vals = [0u32; 3];
    for v in vals.iter_mut() {
        while bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let s = pos;
        while bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        *v = std::str::from_utf8(&bytes[s..pos])
            .unwrap()
            .parse()
            .unwrap();
    }
    pos += 1;
    let (w, h) = (vals[0], vals[1]);
    let n = w as usize * h as usize * ch;
    (bytes[pos..pos + n].to_vec(), w, h, ch)
}

fn decode_all(path: &std::path::Path) -> Vec<u8> {
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
    out
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let d = *x as f64 - *y as f64;
            d * d
        })
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

fn run(px: &[u8], w: u32, h: u32, ch: usize, p: &EncodeParams) -> (usize, f64) {
    let mut buf = Vec::new();
    encode(&mut buf, px, w, h, ch, p).unwrap();
    let tmp = std::env::temp_dir().join(format!("rdb-{}.ecw", std::process::id()));
    std::fs::write(&tmp, &buf).unwrap();
    let got = decode_all(&tmp);
    let _ = std::fs::remove_file(&tmp);
    (buf.len(), psnr(px, &got))
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let (px, w, h, ch) = read_pnm(&std::fs::read(&path).unwrap());
    println!("{path}: {w}x{h}x{ch}\n");
    // What the encoder actually produces for a requested ratio, now that it
    // picks the deadzone by predicted distortion.
    println!(
        "{:>10} {:>10} {:>9} {:>9}",
        "target", "bytes", "ratio", "PSNR dB"
    );
    let raw = px.len() as f64;
    for r in [5u16, 10, 20, 50, 100] {
        let p = EncodeParams {
            target_ratio: Some(r),
            ..EncodeParams::default()
        };
        let (b, q) = run(&px, w, h, ch, &p);
        println!(
            "{:>10} {:>10} {:>8.1}x {:>9.2}",
            format!("{r}:1"),
            b,
            raw / b as f64,
            q
        );
    }
    println!();
    // Bits per PSNR: compare deadzones by bytes needed for a given quality.
    let dzs = [0.5f64, 0.6, 0.7, 0.8, 0.9];
    print!("{:>9}", "strength");
    for dz in dzs {
        print!(" | {:>16}", format!("dz {dz}"));
    }
    println!();
    for s in [4u16, 8, 16, 32, 64] {
        print!("{s:>9}");
        for dz in dzs {
            let p = EncodeParams {
                strength: Some(s),
                deadzone: Some(dz),
                ..EncodeParams::default()
            };
            let (b, q) = run(&px, w, h, ch, &p);
            print!(" | {:>8} {:>7.2}", b, q);
        }
        println!();
    }
}
