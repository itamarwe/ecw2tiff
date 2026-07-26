//! Forward (analysis) wavelet transform — the encode-side inverse of the
//! synthesis in `idwt.rs`.
//!
//! The ECW QMF is separable. Reading the parity cases of the synthesis back
//! out, one level of 1D synthesis from lowpass `A` and highpass `B` to a
//! signal `s` is:
//!
//! ```text
//!   s[2i]   = A(i)   - (B(i-1) + B(i)) / 2
//!   s[2i+1] = (A(i) + A(i+1)) / 2 + B(i)
//! ```
//!
//! where `A(i)`/`B(i)` index the subbands with whole-sample symmetric
//! reflection at the edges (…2,1,0,1,2…), matching the decoder.
//!
//! Analysis inverts that. In the interior, substituting the even equation
//! into the odd one gives a tridiagonal system in `B`
//! (`(B(i-1) + 6B(i) + B(i+1))/4 = s[2i+1] - (s[2i] + s[2i+2])/2`), but the
//! reflected terms at the boundaries fold in extra off-band couplings, so
//! rather than hand-derive those rows we solve the square system
//! `synth(x) = s` iteratively with conjugate gradients on the normal
//! equations, using [`synth_1d`] and its exact adjoint. The operator is the
//! decoder's own synthesis, so analysis cannot silently disagree with it, and
//! the interior relation above makes it well conditioned (a warm start plus a
//! few dozen iterations reaches float-exact round-trip).

/// Reflect index `i` into `[0, n)` with whole-sample symmetry, matching the
/// decoder's edge handling.
#[inline]
fn reflect(i: isize, n: usize) -> usize {
    debug_assert!(n > 0);
    if n == 1 {
        return 0;
    }
    let m = (n - 1) as isize;
    let period = 2 * m;
    let mut k = i.rem_euclid(period);
    if k > m {
        k = period - k;
    }
    k as usize
}

/// Subband lengths for a signal of length `n`: (lowpass, highpass).
#[inline]
pub fn subband_lens(n: usize) -> (usize, usize) {
    (n.div_ceil(2), n / 2)
}

/// One level of 1D synthesis: `a` (len `n.div_ceil(2)`) + `b` (len `n / 2`)
/// -> signal of length `n`.
pub fn synth_1d(a: &[f64], b: &[f64], n: usize) -> Vec<f64> {
    let mut s = vec![0.0f64; n];
    synth_into(a, b, &mut s);
    s
}

fn synth_into(a: &[f64], b: &[f64], s: &mut [f64]) {
    let n = s.len();
    let (m, mb) = subband_lens(n);
    debug_assert_eq!(a.len(), m);
    debug_assert_eq!(b.len(), mb);
    for (idx, sv) in s.iter_mut().enumerate() {
        let i = (idx / 2) as isize;
        if idx % 2 == 0 {
            let mut v = a[reflect(i, m)];
            if mb > 0 {
                v -= (b[reflect(i - 1, mb)] + b[reflect(i, mb)]) / 2.0;
            }
            *sv = v;
        } else {
            *sv = (a[reflect(i, m)] + a[reflect(i + 1, m)]) / 2.0 + b[reflect(i, mb)];
        }
    }
}

/// Adjoint (transpose) of [`synth_1d`]: scatters a residual over the signal
/// back onto the subband coefficients.
fn synth_adjoint(r: &[f64], a_out: &mut [f64], b_out: &mut [f64]) {
    let n = r.len();
    let (m, mb) = subband_lens(n);
    a_out.iter_mut().for_each(|v| *v = 0.0);
    b_out.iter_mut().for_each(|v| *v = 0.0);
    for (idx, &rv) in r.iter().enumerate() {
        let i = (idx / 2) as isize;
        if idx % 2 == 0 {
            a_out[reflect(i, m)] += rv;
            if mb > 0 {
                b_out[reflect(i - 1, mb)] -= rv / 2.0;
                b_out[reflect(i, mb)] -= rv / 2.0;
            }
        } else {
            a_out[reflect(i, m)] += rv / 2.0;
            a_out[reflect(i + 1, m)] += rv / 2.0;
            b_out[reflect(i, mb)] += rv;
        }
    }
}

/// One level of 1D analysis: signal of length `n` -> (`a` len `n.div_ceil(2)`,
/// `b` len `n / 2`). Inverse of [`synth_1d`] to float precision.
pub fn forward_1d(s: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = s.len();
    let (m, mb) = subband_lens(n);
    if mb == 0 {
        return (vec![s[0]], Vec::new()); // n == 1: lone lowpass sample
    }

    // Warm start from the interior relations (exact away from the edges).
    let mut a = vec![0.0f64; m];
    let mut b = vec![0.0f64; mb];
    for i in 0..mb {
        let e0 = s[2 * i];
        let e1 = s[reflect(2 * (i + 1) as isize, n)];
        b[i] = (s[2 * i + 1] - (e0 + e1) / 2.0) * (4.0 / 6.0);
    }
    for i in 0..m {
        let ii = i as isize;
        a[i] = s[2 * i] + (b[reflect(ii - 1, mb)] + b[reflect(ii, mb)]) / 2.0;
    }

    // CG on the normal equations: minimise ||synth(x) - s||.
    let mut resid = vec![0.0f64; n];
    synth_into(&a, &b, &mut resid);
    for (r, sv) in resid.iter_mut().zip(s) {
        *r = sv - *r;
    }

    // g = Aᵀ r  (search direction p starts as g)
    let (mut ga, mut gb) = (vec![0.0f64; m], vec![0.0f64; mb]);
    synth_adjoint(&resid, &mut ga, &mut gb);
    let (mut pa, mut pb) = (ga.clone(), gb.clone());
    let mut g_norm2: f64 =
        ga.iter().map(|v| v * v).sum::<f64>() + gb.iter().map(|v| v * v).sum::<f64>();

    let mut ap = vec![0.0f64; n];
    // n iterations is the exact-arithmetic bound; the cap is a safety net —
    // convergence is reached in far fewer.
    for _ in 0..(2 * n + 16) {
        if g_norm2 <= 1e-26 {
            break;
        }
        synth_into(&pa, &pb, &mut ap);
        let ap_norm2: f64 = ap.iter().map(|v| v * v).sum();
        if ap_norm2 <= f64::MIN_POSITIVE {
            break;
        }
        let alpha = g_norm2 / ap_norm2;
        for (x, d) in a.iter_mut().zip(&pa) {
            *x += alpha * d;
        }
        for (x, d) in b.iter_mut().zip(&pb) {
            *x += alpha * d;
        }
        for (r, apv) in resid.iter_mut().zip(&ap) {
            *r -= alpha * apv;
        }
        synth_adjoint(&resid, &mut ga, &mut gb);
        let new_g2: f64 =
            ga.iter().map(|v| v * v).sum::<f64>() + gb.iter().map(|v| v * v).sum::<f64>();
        let beta = new_g2 / g_norm2;
        g_norm2 = new_g2;
        for (d, g) in pa.iter_mut().zip(&ga) {
            *d = g + beta * *d;
        }
        for (d, g) in pb.iter_mut().zip(&gb) {
            *d = g + beta * *d;
        }
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max_err(a: &[f64], b: &[f64]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max)
    }

    #[test]
    fn round_trip_1d_even_and_odd() {
        for n in [1usize, 2, 3, 4, 5, 8, 9, 16, 17, 64, 65, 100, 101, 512, 513] {
            let s: Vec<f64> = (0..n)
                .map(|i| ((i * 2654435761usize) % 251) as f64 - 125.0)
                .collect();
            let (a, b) = forward_1d(&s);
            assert_eq!((a.len(), b.len()), subband_lens(n));
            let r = synth_1d(&a, &b, n);
            let e = max_err(&s, &r);
            assert!(e < 1e-6, "n={n} max err {e}");
        }
    }

    #[test]
    fn round_trip_smooth_and_constant() {
        // Smooth and constant signals: highpass should be ~0 for a constant.
        let n = 128;
        let s: Vec<f64> = (0..n)
            .map(|i| 50.0 + (i as f64 * 0.05).sin() * 30.0)
            .collect();
        let (a, b) = forward_1d(&s);
        assert!(max_err(&s, &synth_1d(&a, &b, n)) < 1e-6);

        let c = vec![77.0f64; n];
        let (a, b) = forward_1d(&c);
        assert!(
            b.iter().all(|v| v.abs() < 1e-6),
            "constant signal has nonzero highpass"
        );
        assert!(max_err(&c, &synth_1d(&a, &b, n)) < 1e-6);
    }

    #[test]
    fn synth_forward_are_inverse_1d() {
        for n in [4usize, 5, 16, 17, 64] {
            let (m, mb) = subband_lens(n);
            let a: Vec<f64> = (0..m).map(|i| (i as f64 * 1.7).sin() * 40.0).collect();
            let b: Vec<f64> = (0..mb).map(|i| (i as f64 * 0.9).cos() * 25.0).collect();
            let s = synth_1d(&a, &b, n);
            let (a2, b2) = forward_1d(&s);
            assert!(max_err(&a, &a2) < 1e-6 && max_err(&b, &b2) < 1e-6, "n={n}");
        }
    }

    #[test]
    fn adjoint_matches_operator() {
        // <A x, y> == <x, Aᵀ y> for random x, y.
        let n = 33;
        let (m, mb) = subband_lens(n);
        let a: Vec<f64> = (0..m).map(|i| ((i * 7919) % 97) as f64 - 48.0).collect();
        let b: Vec<f64> = (0..mb).map(|i| ((i * 6151) % 89) as f64 - 44.0).collect();
        let y: Vec<f64> = (0..n).map(|i| ((i * 3571) % 83) as f64 - 41.0).collect();
        let lhs: f64 = synth_1d(&a, &b, n).iter().zip(&y).map(|(u, v)| u * v).sum();
        let (mut ta, mut tb) = (vec![0.0; m], vec![0.0; mb]);
        synth_adjoint(&y, &mut ta, &mut tb);
        let rhs: f64 = a.iter().zip(&ta).map(|(u, v)| u * v).sum::<f64>()
            + b.iter().zip(&tb).map(|(u, v)| u * v).sum::<f64>();
        assert!((lhs - rhs).abs() < 1e-9, "adjoint mismatch: {lhs} vs {rhs}");
    }
}
