//! Measure how a unit coefficient in each (level, subband) propagates to
//! pixel-domain error, by pushing an impulse through the synthesis chain.
//!
//! These gains are the correct relative weighting for quantization: a bin of
//! size b in a band with gain g contributes error proportional to g*b, so
//! equalizing distortion across bands means bins proportional to 1/g.
use ecw2tiff::fdwt::{synth_2d, Plane, Subbands};

fn zeros(w: usize, h: usize) -> Plane {
    Plane::new(w, h)
}

/// L2 norm of the image produced by a single unit coefficient at
/// (level, band). band: 0=LL (level 0 only), 1=LH, 2=HL, 3=HH.
fn gain(levels: usize, base: usize, level: usize, band: usize) -> f64 {
    // Subband size at level L is base << L (level 0 smallest).
    let mut cur = {
        let s = base;
        let mut ll = zeros(s, s);
        if level == 0 && band == 0 {
            ll.set(s / 2, s / 2, 1.0);
        }
        ll
    };
    for l in 0..levels {
        let s = base << l;
        let mut lh = zeros(s, s);
        let mut hl = zeros(s, s);
        let mut hh = zeros(s, s);
        if l == level {
            match band {
                1 => lh.set(s / 2, s / 2, 1.0),
                2 => hl.set(s / 2, s / 2, 1.0),
                3 => hh.set(s / 2, s / 2, 1.0),
                _ => {}
            }
        }
        cur = synth_2d(&Subbands {
            ll: cur,
            lh,
            hl,
            hh,
        });
    }
    cur.data.iter().map(|v| v * v).sum::<f64>().sqrt()
}

fn main() {
    // Does gain depend only on depth below the image, or on total levels?
    println!("depth-invariance check (gain of LH at depth d, for several pyramid depths):");
    for levels in [2usize, 3, 4, 5, 6] {
        let g: Vec<String> = (0..levels)
            .map(|l| format!("{:.3}", gain(levels, 4, l, 1)))
            .collect();
        // print finest-first so equal depths line up
        let mut g = g;
        g.reverse();
        println!("  levels={levels}: by depth 0..  {}", g.join("  "));
    }
    println!();

    let levels = 5;
    let base = 8; // level 0 is 8x8, image ends up 256x256
    println!(
        "levels={levels}, level 0 = {base}x{base}, image = {}x{}",
        base << levels,
        base << levels
    );
    println!(
        "\n{:<8} {:>10} {:>10} {:>10} {:>10}",
        "level", "LL", "LH", "HL", "HH"
    );
    let ll = gain(levels, base, 0, 0);
    for l in 0..levels {
        let g: Vec<f64> = (1..=3).map(|b| gain(levels, base, l, b)).collect();
        let llc = if l == 0 {
            format!("{ll:>10.3}")
        } else {
            format!("{:>10}", "-")
        };
        println!(
            "{l:<8} {} {:>10.3} {:>10.3} {:>10.3}",
            llc, g[0], g[1], g[2]
        );
    }
    println!("\nratios relative to the finest level's LH:");
    let fine = gain(levels, base, levels - 1, 1);
    for l in 0..levels {
        let g: Vec<f64> = (1..=3).map(|b| gain(levels, base, l, b) / fine).collect();
        println!(
            "  level {l}: LH={:.3}  HL={:.3}  HH={:.3}",
            g[0], g[1], g[2]
        );
    }
    println!("  level 0 LL = {:.3}", ll / fine);
}
