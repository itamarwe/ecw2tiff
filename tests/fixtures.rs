//! Decode checks against the synthetic fixtures in `tests/data`.
//!
//! `synth_*.ecw` are generated files (random coefficients, every entropy
//! encoding, empty tiles, partial edge tiles, 3-5 stage pyramids). Their
//! expected outputs in `cases.txt` are SHA-256 digests of the decoded
//! window, recorded independently of this decoder.

use ecw2tiff::codec::{EcwImage, Window};
use std::path::{Path, PathBuf};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn decode(img: &EcwImage, w: Window, first: u32, n: u32) -> Vec<u8> {
    let row = w.out_width as usize * img.info().channels();
    let mut out = vec![0u8; row * n as usize];
    let mut r = img.rows(w, first, n).expect("rows");
    for chunk in out.chunks_exact_mut(row) {
        r.read_row(chunk).expect("read_row");
    }
    out
}

fn window(x: u32, y: u32, w: u32, h: u32, nx: u32, ny: u32) -> Window {
    Window {
        x0: x,
        y0: y,
        x1: x + w - 1,
        y1: y + h - 1,
        out_width: nx,
        out_height: ny,
    }
}

#[test]
fn recorded_cases_match() {
    let list = std::fs::read_to_string(data("cases.txt")).unwrap();
    let mut n = 0;
    for line in list.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let v: Vec<u32> = f[1..7].iter().map(|s| s.parse().unwrap()).collect();
        let img = EcwImage::open(&data(f[0])).unwrap();
        let w = window(v[0], v[1], v[2], v[3], v[4], v[5]);
        let got = sha256_hex(&decode(&img, w, 0, v[5]));
        assert_eq!(got, f[7], "{line}");
        n += 1;
    }
    assert!(n >= 60);
}

#[test]
fn strips_equal_full_decode() {
    for name in ["synth_ycc.ecw", "synth_grey.ecw", "synth_multi.ecw"] {
        let img = EcwImage::open(&data(name)).unwrap();
        let (w, h) = (img.info().width, img.info().height);
        for (nx, ny) in [(w, h), (w / 3, h / 3), (w / 2 + 1, h / 5)] {
            let win = window(0, 0, w, h, nx, ny);
            let full = decode(&img, win, 0, ny);
            let row = nx as usize * img.info().channels();
            let mut first = 0;
            let mut step = 1;
            while first < ny {
                let n = step.min(ny - first);
                let part = decode(&img, win, first, n);
                let at = first as usize * row;
                assert_eq!(
                    part,
                    full[at..at + part.len()],
                    "{name} {nx}x{ny} rows {first}+{n}"
                );
                first += n;
                step = step * 2 + 1;
            }
        }
    }
}

#[test]
fn concurrent_readers_share_one_image() {
    let img = EcwImage::open(&data("synth_ycc.ecw")).unwrap();
    let (w, h) = (img.info().width, img.info().height);
    let win = window(0, 0, w, h, w, h);
    let full = decode(&img, win, 0, h);
    let row = w as usize * 3;
    let parts: Vec<(u32, Vec<u8>)> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..4)
            .map(|k| {
                let img = &img;
                s.spawn(move || {
                    let first = k * h / 4;
                    let n = (k + 1) * h / 4 - first;
                    (first, decode(img, win, first, n))
                })
            })
            .collect();
        hs.into_iter().map(|t| t.join().unwrap()).collect()
    });
    for (first, part) in parts {
        let at = first as usize * row;
        assert_eq!(part, full[at..at + part.len()]);
    }
}

#[test]
fn corrupted_files_never_panic() {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let tmp = std::env::temp_dir().join(format!("ecw2tiff-fuzz-{}.ecw", std::process::id()));
    for name in ["alt16.ecw", "synth_grey.ecw", "synth_multi.ecw"] {
        let orig = std::fs::read(data(name)).unwrap();
        for i in 0..150 {
            let mut d = orig.clone();
            if i % 3 == 0 {
                d.truncate((rnd() as usize) % d.len());
            } else {
                for _ in 0..1 + rnd() % 8 {
                    let at = if i % 3 == 1 {
                        rnd() as usize % d.len().min(400)
                    } else {
                        rnd() as usize % d.len()
                    };
                    d[at] = rnd() as u8;
                }
            }
            std::fs::write(&tmp, &d).unwrap();
            let Ok(img) = EcwImage::open(&tmp) else {
                continue;
            };
            let info = img.info().clone();
            if info.width == 0 || info.height == 0 || info.width > 4096 || info.height > 4096 {
                continue;
            }
            let win = window(
                0,
                0,
                info.width,
                info.height,
                info.width.min(97),
                info.height.min(61),
            );
            if let Ok(mut r) = img.rows(win, 0, win.out_height) {
                let mut buf = vec![0u8; win.out_width as usize * info.channels()];
                for _ in 0..win.out_height {
                    if r.read_row(&mut buf).is_err() {
                        break;
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
}

// --- minimal SHA-256 for digest comparison (FIPS 180-4) ---

fn sha256_hex(msg: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut m = msg.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&((msg.len() as u64) * 8).to_be_bytes());
    for block in m.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, c) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}
