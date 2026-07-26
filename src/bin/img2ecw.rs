//! `img2ecw` — encode a binary PGM/PPM (Netpbm P5/P6) image as ECW v2.
//!
//! PNM keeps the CLI dependency-free while still accepting real images:
//! most tools can produce it (`gdal_translate -of PNM`, ImageMagick
//! `convert`, `ffmpeg`). Decode the result with `ecw2tiff`.

use ecw2tiff::encode::{encode, max_levels, EncodeParams};
use std::io::{BufWriter, Read};
use std::process::exit;

fn usage() -> ! {
    eprintln!(
        "img2ecw - encode a PGM/PPM image as ECW v2 (from-scratch encoder)

USAGE:
    img2ecw [OPTIONS] <input.pgm|input.ppm> <output.ecw>

OPTIONS:
    --levels <n>       pyramid levels (default: as deep as the size allows)
    --block <n>        block size, multiple of 8 (default 64)
    --ratio <n>        lossy: target compression ratio (e.g. 20). Omit for
                       lossless encoding
    --strength <n>     lossy: quantization strength directly, skipping the
                       size search (higher = smaller and coarser)
    --deadzone <f>     zero-bin width in steps (0.5 = plain rounding).
                       Default: chosen per encode by predicted error
    --scale <n>        lossless quantization scale factor (default 16)
    --bin <n>          quantization bin size (default 1)
    --origin <x,y>     georeferencing origin (default 0,0)
    --cell <x,y>       cell size (default 1,-1)
    --projection <s>   projection name, e.g. GEODETIC (default RAW)
    --datum <s>        datum name, e.g. WGS84 (default RAW)

Dimensions must be divisible by 2^levels.
"
    );
    exit(2);
}

/// Minimal binary Netpbm reader: P5 (grey) and P6 (RGB), maxval 255.
fn read_pnm(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32, usize), String> {
    if bytes.len() < 2 || bytes[0] != b'P' {
        return Err("not a Netpbm file".into());
    }
    let ch = match bytes[1] {
        b'5' => 1,
        b'6' => 3,
        b'2' | b'3' => return Err("ASCII PNM (P2/P3) is not supported; use binary P5/P6".into()),
        c => return Err(format!("unsupported Netpbm type P{}", c as char)),
    };
    // Three whitespace-separated integers (width, height, maxval), with
    // '#' comments allowed between them.
    let mut pos = 2usize;
    let mut vals = [0u32; 3];
    for v in vals.iter_mut() {
        loop {
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if pos < bytes.len() && bytes[pos] == b'#' {
                while pos < bytes.len() && bytes[pos] != b'\n' {
                    pos += 1;
                }
            } else {
                break;
            }
        }
        let start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        if start == pos {
            return Err("malformed Netpbm header".into());
        }
        *v = std::str::from_utf8(&bytes[start..pos])
            .unwrap()
            .parse()
            .map_err(|_| "bad number in Netpbm header".to_string())?;
    }
    if vals[2] != 255 {
        return Err(format!(
            "only 8-bit Netpbm is supported (maxval {})",
            vals[2]
        ));
    }
    pos += 1; // single whitespace byte after maxval
    let (w, h) = (vals[0], vals[1]);
    let need = w as usize * h as usize * ch;
    if bytes.len() < pos + need {
        return Err(format!(
            "truncated pixel data: expected {need} bytes, found {}",
            bytes.len().saturating_sub(pos)
        ));
    }
    Ok((bytes[pos..pos + need].to_vec(), w, h, ch))
}

fn parse_pair(s: &str) -> (f64, f64) {
    let mut it = s.split(',').map(|v| v.trim().parse::<f64>());
    match (it.next(), it.next()) {
        (Some(Ok(a)), Some(Ok(b))) => (a, b),
        _ => usage(),
    }
}

fn main() {
    let mut p = EncodeParams::default();
    let mut pos: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut next = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--levels" => p.levels = next().parse().unwrap_or_else(|_| usage()),
            "--block" => p.block_size = next().parse().unwrap_or_else(|_| usage()),
            "--scale" => p.scale_factor = next().parse().unwrap_or_else(|_| usage()),
            "--ratio" => {
                let r: u16 = next().parse().unwrap_or_else(|_| usage());
                if r == 0 {
                    eprintln!("error: --ratio must be at least 1");
                    exit(2);
                }
                p.target_ratio = Some(r);
            }
            "--deadzone" => {
                let d: f64 = next().parse().unwrap_or_else(|_| usage());
                if !(0.5..=2.0).contains(&d) {
                    eprintln!("error: --deadzone must be between 0.5 and 2.0");
                    exit(2);
                }
                p.deadzone = Some(d);
            }
            "--strength" => p.strength = Some(next().parse().unwrap_or_else(|_| usage())),
            "--bin" => p.bin_size = next().parse().unwrap_or_else(|_| usage()),
            "--origin" => {
                let (x, y) = parse_pair(&next());
                p.origin_x = x;
                p.origin_y = y;
            }
            "--cell" => {
                let (x, y) = parse_pair(&next());
                p.cell_inc_x = x;
                p.cell_inc_y = y;
                p.cell_units = 2;
            }
            "--projection" => p.projection = next(),
            "--datum" => p.datum = next(),
            "-h" | "--help" => usage(),
            _ => pos.push(a),
        }
    }
    if pos.len() != 2 {
        usage();
    }

    let mut raw = Vec::new();
    match std::fs::File::open(&pos[0]).and_then(|mut f| f.read_to_end(&mut raw)) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", pos[0]);
            exit(1);
        }
    }
    let (pixels, w, h, ch) = match read_pnm(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {}: {e}", pos[0]);
            exit(1);
        }
    };
    if p.levels == 0 && max_levels(w, h) == 0 {
        eprintln!("error: {w}x{h} has an odd dimension; ECW levels need even sizes");
        exit(1);
    }

    let file = match std::fs::File::create(&pos[1]) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: cannot create {}: {e}", pos[1]);
            exit(1);
        }
    };
    let mut out = BufWriter::new(file);
    if let Err(e) = encode(&mut out, &pixels, w, h, ch, &p) {
        eprintln!("error: encode failed: {e}");
        exit(1);
    }
    let levels = if p.levels == 0 {
        max_levels(w, h)
    } else {
        p.levels
    };
    eprintln!(
        "wrote {} ({}x{}, {} band{}, {levels} levels, {})",
        pos[1],
        w,
        h,
        ch,
        if ch == 1 { "" } else { "s" },
        match p.target_ratio {
            Some(r) => format!("lossy, target {r}:1"),
            None => "lossless".to_string(),
        }
    );
}
