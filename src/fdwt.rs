//! Forward (analysis) wavelet transform — the encode-side inverse of the
//! synthesis in `idwt.rs`.
//!
//! # The decoder's transform
//!
//! The ECW QMF is separable, and the decoder extends every subband by one
//! sample on each side by **edge replication** (`line1[0] = line1[1]` and
//! friends in `pull_line`), i.e. `X(-1) = X(0)` and `X(m) = X(m-1)`.
//!
//! One level of 1D synthesis from lowpass `a` and highpass `b` (both length
//! `m`) to a signal of length `2m` is:
//!
//! ```text
//!   s[2i]   = a(i) - (b(i-1) + b(i)) / 2
//!   s[2i+1] = (a(i) + a(i+1)) / 2 + b(i)
//! ```
//!
//! The 2D synthesis is that applied separably, with `LH` highpass in **y** and
//! `HL` highpass in **x**; [`synth_2d`] does the separable version and
//! [`synth_2d_reference`] transcribes the decoder's four fused parity
//! expressions literally, with a test asserting the two agree.
//!
//! # Analysis
//!
//! Substituting the even equation into the odd one eliminates `a` and leaves a
//! tridiagonal system in `b`:
//!
//! ```text
//!   b(i-1) + 6 b(i) + b(i+1) = 4 (s[2i+1] - (s[2i] + s[2i+2]) / 2)
//! ```
//!
//! Replication folds the reflected neighbour onto the diagonal at `i = 0`
//! (`b(-1) = b(0)`, diagonal 6 -> 7) and, at `i = m-1`, clamping `a(m) = a(m-1)`
//! changes the row to `2 b(m-2) + 6 b(m-1) = 4 (s[2m-1] - s[2m-2])`. The system
//! is diagonally dominant, so a Thomas solve recovers `b` exactly in O(m);
//! then `a(i) = s[2i] + (b(i-1) + b(i)) / 2`.
//!
//! Only even lengths are handled: every ECW level we produce is exactly half
//! its parent, so all subband lengths are `n / 2`.

/// Clamp an index into `[0, n)` — the decoder's one-sample edge replication.
#[inline]
fn cl(i: isize, n: usize) -> usize {
    i.clamp(0, n as isize - 1) as usize
}

/// One level of 1D synthesis: `a`, `b` (both length `m`) -> signal of length
/// `2m`.
pub fn synth_1d(a: &[f64], b: &[f64]) -> Vec<f64> {
    let m = a.len();
    assert_eq!(b.len(), m, "subbands must have equal length");
    let mut s = vec![0.0f64; 2 * m];
    for i in 0..m {
        let ii = i as isize;
        s[2 * i] = a[i] - (b[cl(ii - 1, m)] + b[i]) / 2.0;
        s[2 * i + 1] = (a[i] + a[cl(ii + 1, m)]) / 2.0 + b[i];
    }
    s
}

/// One level of 1D analysis: signal of even length `2m` -> (`a`, `b`), both
/// length `m`. Exact inverse of [`synth_1d`].
pub fn forward_1d(s: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = s.len();
    assert!(n >= 2 && n % 2 == 0, "signal length must be even and >= 2");
    let m = n / 2;
    if m == 1 {
        // a(1)=a(0), b(-1)=b(0): s[0] = a-b, s[1] = a+b.
        return (vec![(s[0] + s[1]) / 2.0], vec![(s[1] - s[0]) / 2.0]);
    }

    // Tridiagonal system for b (see module docs).
    let mut diag = vec![6.0f64; m];
    let mut lower = vec![1.0f64; m];
    let mut upper = vec![1.0f64; m];
    lower[0] = 0.0;
    upper[m - 1] = 0.0;
    diag[0] = 7.0; // b(-1) = b(0)
    lower[m - 1] = 2.0; // a(m) = a(m-1)

    let mut rhs = vec![0.0f64; m];
    for i in 0..m - 1 {
        rhs[i] = 4.0 * (s[2 * i + 1] - (s[2 * i] + s[2 * i + 2]) / 2.0);
    }
    rhs[m - 1] = 4.0 * (s[n - 1] - s[n - 2]);

    // Thomas algorithm.
    let mut cp = vec![0.0f64; m];
    let mut dp = vec![0.0f64; m];
    cp[0] = upper[0] / diag[0];
    dp[0] = rhs[0] / diag[0];
    for i in 1..m {
        let denom = diag[i] - lower[i] * cp[i - 1];
        cp[i] = upper[i] / denom;
        dp[i] = (rhs[i] - lower[i] * dp[i - 1]) / denom;
    }
    let mut b = vec![0.0f64; m];
    b[m - 1] = dp[m - 1];
    for i in (0..m - 1).rev() {
        b[i] = dp[i] - cp[i] * b[i + 1];
    }

    let a: Vec<f64> = (0..m)
        .map(|i| s[2 * i] + (b[cl(i as isize - 1, m)] + b[i]) / 2.0)
        .collect();
    (a, b)
}

/// A 2D subband plane: `mw` x `mh` f64 coefficients, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f64>,
}

impl Plane {
    pub fn new(w: usize, h: usize) -> Plane {
        Plane {
            w,
            h,
            data: vec![0.0; w * h],
        }
    }
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f64 {
        self.data[y * self.w + x]
    }
    /// Edge-replicating fetch, matching the decoder's boundary handling.
    #[inline]
    pub fn atc(&self, x: isize, y: isize) -> f64 {
        self.data[cl(y, self.h) * self.w + cl(x, self.w)]
    }
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: f64) {
        self.data[y * self.w + x] = v;
    }
    pub fn from_vec(w: usize, h: usize, data: Vec<f64>) -> Plane {
        assert_eq!(data.len(), w * h);
        Plane { w, h, data }
    }
}

/// The four subbands of one decomposition level. Each is `mw` x `mh`; together
/// they synthesize a `2mw` x `2mh` plane. `lh` is highpass in y, `hl` in x.
#[derive(Clone, Debug)]
pub struct Subbands {
    pub ll: Plane,
    pub lh: Plane,
    pub hl: Plane,
    pub hh: Plane,
}

/// Separable 2D synthesis: y pass then x pass. Produces `2mw` x `2mh`.
pub fn synth_2d(sb: &Subbands) -> Plane {
    let (mw, mh) = (sb.ll.w, sb.ll.h);
    let (w, h) = (2 * mw, 2 * mh);
    // y pass: (ll, lh) -> u (low in x), (hl, hh) -> v (high in x); both full h.
    let mut u = Plane::new(mw, h);
    let mut v = Plane::new(mw, h);
    let mut col_lo = vec![0.0f64; mh];
    let mut col_hi = vec![0.0f64; mh];
    for x in 0..mw {
        for y in 0..mh {
            col_lo[y] = sb.ll.at(x, y);
            col_hi[y] = sb.lh.at(x, y);
        }
        let s = synth_1d(&col_lo, &col_hi);
        for (y, &sv) in s.iter().enumerate() {
            u.set(x, y, sv);
        }
        for y in 0..mh {
            col_lo[y] = sb.hl.at(x, y);
            col_hi[y] = sb.hh.at(x, y);
        }
        let s = synth_1d(&col_lo, &col_hi);
        for (y, &sv) in s.iter().enumerate() {
            v.set(x, y, sv);
        }
    }
    // x pass: (u, v) -> output row.
    let mut out = Plane::new(w, h);
    for y in 0..h {
        let row_lo = &u.data[y * mw..(y + 1) * mw];
        let row_hi = &v.data[y * mw..(y + 1) * mw];
        let s = synth_1d(row_lo, row_hi);
        out.data[y * w..(y + 1) * w].copy_from_slice(&s);
    }
    out
}

/// Literal transcription of the decoder's four fused parity expressions from
/// `idwt.rs::pull_line`, for cross-checking [`synth_2d`]. Subband column `i`
/// corresponds to the decoder's padded index `b + 1`.
///
/// The line pairing follows the decoder's `read_lines` bookkeeping: an even
/// output line synthesizes from subband rows `(j-1, j)`, then sets
/// `read_lines = 1` so the following odd output line has pulled one more input
/// line and synthesizes from `(j, j+1)`. Rows are edge-replicated.
pub fn synth_2d_reference(sb: &Subbands) -> Plane {
    let (mw, mh) = (sb.ll.w, sb.ll.h);
    let (w, h) = (2 * mw, 2 * mh);
    let mut out = Plane::new(w, h);
    for y in 0..h {
        let j = (y / 2) as isize;
        let (r0, r1) = if y % 2 == 0 { (j - 1, j) } else { (j, j + 1) };
        let ll0 = |i: isize| sb.ll.atc(i, r0);
        let lh0 = |i: isize| sb.lh.atc(i, r0);
        let hl0 = |i: isize| sb.hl.atc(i, r0);
        let hh0 = |i: isize| sb.hh.atc(i, r0);
        let ll1 = |i: isize| sb.ll.atc(i, r1);
        let lh1 = |i: isize| sb.lh.atc(i, r1);
        let hl1 = |i: isize| sb.hl.atc(i, r1);
        let hh1 = |i: isize| sb.hh.atc(i, r1);
        for x in 0..w {
            let i = (x / 2) as isize;
            let v = if y % 2 == 0 {
                if x % 2 == 0 {
                    ll1(i) - (lh0(i) + lh1(i) + hl1(i - 1) + hl1(i)) * 0.5
                        + (hh0(i - 1) + hh0(i) + hh1(i - 1) + hh1(i)) * 0.25
                } else {
                    ((ll1(i) + ll1(i + 1)) - (hh0(i) + hh1(i))) * 0.5
                        - (lh0(i) + lh0(i + 1) + lh1(i) + lh1(i + 1)) * 0.25
                        + hl1(i)
                }
            } else if x % 2 == 0 {
                ((ll0(i) + ll1(i)) - (hh0(i - 1) + hh0(i))) * 0.5 + lh0(i)
                    - (hl0(i - 1) + hl0(i) + hl1(i - 1) + hl1(i)) * 0.25
            } else {
                (ll0(i) + ll0(i + 1) + ll1(i) + ll1(i + 1)) * 0.25
                    + (lh0(i) + lh0(i + 1) + hl0(i) + hl1(i)) * 0.5
                    + hh0(i)
            };
            out.set(x, y, v);
        }
    }
    out
}

/// 2D analysis: split a plane with even dimensions into its four subbands.
/// Exact inverse of [`synth_2d`].
pub fn forward_2d(p: &Plane) -> Subbands {
    assert!(
        p.w >= 2 && p.h >= 2 && p.w % 2 == 0 && p.h % 2 == 0,
        "plane dimensions must be even and >= 2 (got {}x{})",
        p.w,
        p.h
    );
    let (mw, mh) = (p.w / 2, p.h / 2);
    // x pass: each row -> low-x | high-x
    let mut lo_x = Plane::new(mw, p.h);
    let mut hi_x = Plane::new(mw, p.h);
    for y in 0..p.h {
        let (a, b) = forward_1d(&p.data[y * p.w..(y + 1) * p.w]);
        lo_x.data[y * mw..(y + 1) * mw].copy_from_slice(&a);
        hi_x.data[y * mw..(y + 1) * mw].copy_from_slice(&b);
    }
    // y pass on each: low-y | high-y
    let mut ll = Plane::new(mw, mh);
    let mut lh = Plane::new(mw, mh);
    let mut hl = Plane::new(mw, mh);
    let mut hh = Plane::new(mw, mh);
    let mut col = vec![0.0f64; p.h];
    for x in 0..mw {
        for y in 0..p.h {
            col[y] = lo_x.at(x, y);
        }
        let (a, b) = forward_1d(&col);
        for y in 0..mh {
            ll.set(x, y, a[y]);
            lh.set(x, y, b[y]);
        }
        for y in 0..p.h {
            col[y] = hi_x.at(x, y);
        }
        let (a, b) = forward_1d(&col);
        for y in 0..mh {
            hl.set(x, y, a[y]);
            hh.set(x, y, b[y]);
        }
    }
    Subbands { ll, lh, hl, hh }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max_err(a: &[f64], b: &[f64]) -> f64 {
        assert_eq!(a.len(), b.len());
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max)
    }

    fn pseudo(n: usize, salt: usize) -> Vec<f64> {
        (0..n)
            .map(|i| (((i + 1) * (2654435761 + salt * 7919)) % 251) as f64 - 125.0)
            .collect()
    }

    #[test]
    fn round_trip_1d() {
        for m in [1usize, 2, 3, 4, 8, 9, 32, 64, 256] {
            let s = pseudo(2 * m, m);
            let (a, b) = forward_1d(&s);
            assert_eq!((a.len(), b.len()), (m, m));
            let e = max_err(&s, &synth_1d(&a, &b));
            assert!(e < 1e-9, "m={m} max err {e}");
        }
    }

    #[test]
    fn forward_inverts_synth_1d() {
        for m in [1usize, 2, 5, 16, 64] {
            let a = pseudo(m, 1);
            let b = pseudo(m, 2);
            let (a2, b2) = forward_1d(&synth_1d(&a, &b));
            assert!(max_err(&a, &a2) < 1e-9 && max_err(&b, &b2) < 1e-9, "m={m}");
        }
    }

    #[test]
    fn constant_signal_has_zero_highpass() {
        let c = vec![42.0f64; 64];
        let (a, b) = forward_1d(&c);
        assert!(b.iter().all(|v| v.abs() < 1e-9));
        assert!(a.iter().all(|v| (v - 42.0).abs() < 1e-9));
    }

    /// The separable implementation must equal the decoder's fused formulas.
    #[test]
    fn separable_matches_decoder_formulas() {
        for (mw, mh) in [(1usize, 1usize), (2, 3), (4, 4), (5, 7), (16, 9)] {
            let sb = Subbands {
                ll: Plane::from_vec(mw, mh, pseudo(mw * mh, 1)),
                lh: Plane::from_vec(mw, mh, pseudo(mw * mh, 2)),
                hl: Plane::from_vec(mw, mh, pseudo(mw * mh, 3)),
                hh: Plane::from_vec(mw, mh, pseudo(mw * mh, 4)),
            };
            let sep = synth_2d(&sb);
            let refr = synth_2d_reference(&sb);
            let e = max_err(&sep.data, &refr.data);
            assert!(e < 1e-9, "{mw}x{mh}: separable vs decoder formulas err {e}");
        }
    }

    #[test]
    fn round_trip_2d() {
        for (w, h) in [(2usize, 2usize), (4, 6), (16, 16), (32, 8), (64, 64)] {
            let p = Plane::from_vec(w, h, pseudo(w * h, 5));
            let sb = forward_2d(&p);
            assert_eq!((sb.ll.w, sb.ll.h), (w / 2, h / 2));
            let r = synth_2d(&sb);
            let e = max_err(&p.data, &r.data);
            assert!(e < 1e-9, "{w}x{h} round-trip err {e}");
            // and against the decoder's own formulas
            let e2 = max_err(&p.data, &synth_2d_reference(&sb).data);
            assert!(e2 < 1e-9, "{w}x{h} decoder-formula round-trip err {e2}");
        }
    }

    #[test]
    fn flat_image_puts_energy_only_in_ll() {
        let p = Plane::from_vec(32, 32, vec![100.0; 32 * 32]);
        let sb = forward_2d(&p);
        assert!(sb.lh.data.iter().all(|v| v.abs() < 1e-9));
        assert!(sb.hl.data.iter().all(|v| v.abs() < 1e-9));
        assert!(sb.hh.data.iter().all(|v| v.abs() < 1e-9));
        assert!(sb.ll.data.iter().all(|v| (v - 100.0).abs() < 1e-9));
    }
}
