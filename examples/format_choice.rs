//! Why does the range coder gain so little? Compare candidate encodings on
//! streams shaped like real sidebands (one 64x64 block = 4096 coefficients).
use ecw2tiff::sbencode::candidate_sizes;

fn name(f: u8) -> &'static str {
    match f {
        1 => "RAW",
        2 => "HUFFMAN",
        3 => "RANGE",
        4 => "RANGE8",
        5 => "ZEROS",
        6 => "RUN_ZERO",
        _ => "?",
    }
}

fn report(label: &str, v: &[i16]) {
    let mut c = candidate_sizes(v);
    c.sort_by_key(|&(_, n)| n);
    let best = c[0].1;
    let parts: Vec<String> = c
        .iter()
        .map(|&(f, n)| format!("{}={}{}", name(f), n, if n == best { "*" } else { "" }))
        .collect();
    println!("  {label:<34} n={:<6} {}", v.len(), parts.join("  "));
}

fn main() {
    // Quantized detail bands are dominated by zeros; density rises as
    // quantization gets finer.
    for (label, keep) in [
        ("detail, 2% non-zero", 50u64),
        ("detail, 10% non-zero", 10),
        ("detail, 50% non-zero", 2),
    ] {
        let v: Vec<i16> = (0..4096)
            .map(|i| {
                let x = (i as u64 + 1).wrapping_mul(6364136223846793005);
                if !(x >> 33).is_multiple_of(keep) {
                    0
                } else {
                    ((x >> 20) % 21) as i16 - 10
                }
            })
            .collect();
        report(label, &v);
    }
    // LL band: smooth, large magnitudes.
    let ll: Vec<i16> = (0..4096)
        .map(|i| {
            let (x, y) = ((i % 64) as f64, (i / 64) as f64);
            (2000.0 + 900.0 * ((x / 9.0).sin() + (y / 7.0).cos())) as i16
        })
        .collect();
    report("LL band (smooth, large)", &ll);

    // Smaller streams: deep pyramid levels have tiny blocks, where an
    // adaptive model has little data to learn from.
    for n in [64usize, 256, 1024] {
        let v: Vec<i16> = (0..n)
            .map(|i| {
                let x = (i as u64 + 1).wrapping_mul(6364136223846793005);
                if !(x >> 33).is_multiple_of(10) {
                    0
                } else {
                    ((x >> 20) % 21) as i16 - 10
                }
            })
            .collect();
        report(&format!("detail, 10% non-zero, n={n}"), &v);
    }
}
