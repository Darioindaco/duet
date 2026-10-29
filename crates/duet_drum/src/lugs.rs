//! Lug tension non-uniformity (§4.5).
//!
//! Tension varies around the head: each lug adds a bump (a von Mises function, mean 1) of its
//! relative deviation `d_l`:
//!
//! `δT(θ) = T_h · (1/N_lug) · Σ_l d_l · B(θ − θ_l)`,  `B(φ) = exp(s·cos φ)/I_0(s)`,  `θ_l = θ_lug0 + 2πl/N_lug`
//!
//! Its Fourier coefficients `δT = c_0 + Σ_p (c_p cos pθ + s_p sin pθ)` do two things: `c_0` is a mean
//! shift of the tension (the same for every mode of the head), and `c_2m, s_2m` split the degenerate
//! (m, n) pair (first-order degenerate perturbation theory; no coupling between different m).
//! The tension is assumed to vary with θ only, not with r.

use std::f64::consts::PI;

use crate::bessel::bessel_i0;

/// The DFT size, and with it the harmonics we have (1 ..= POINTS/2 − 1).
const POINTS: usize = 1024;

#[derive(Clone, Debug)]
pub struct LugProfile {
    /// the mean shift of the tension (N/m)
    pub c0: f64,
    c: Vec<f64>,
    s: Vec<f64>,
}

impl LugProfile {
    /// The profile of a head with these lug deviations, or `None` when every lug is nominal.
    pub fn new(tension: f64, deviations: &[f64], n_lug: usize, theta0: f64, sharpness: f64) -> Option<Self> {
        if deviations.iter().all(|d| *d == 0.0) {
            return None;
        }
        assert_eq!(deviations.len(), n_lug, "one deviation per lug");
        let i0 = bessel_i0(sharpness);
        let theta = |j: usize| 2.0 * PI * j as f64 / POINTS as f64;
        let dt: Vec<f64> = (0..POINTS)
            .map(|j| {
                let mut sum = 0.0;
                for (l, d) in deviations.iter().enumerate() {
                    let tl = theta0 + 2.0 * PI * l as f64 / n_lug as f64;
                    sum += d * (sharpness * (theta(j) - tl).cos()).exp() / i0;
                }
                tension * sum / n_lug as f64
            })
            .collect();
        let c0 = dt.iter().sum::<f64>() / POINTS as f64;
        let (mut c, mut s) = (vec![0.0; POINTS / 2], vec![0.0; POINTS / 2]);
        // (the sines and cosines of p·θ_j are table lookups: p·j mod POINTS)
        let tab: Vec<(f64, f64)> = (0..POINTS).map(|i| (theta(i).cos(), theta(i).sin())).collect();
        for p in 1..POINTS / 2 {
            let (mut a, mut b) = (0.0, 0.0);
            for (j, v) in dt.iter().enumerate() {
                let (cs, sn) = tab[(p * j) % POINTS];
                a += v * cs;
                b += v * sn;
            }
            c[p] = 2.0 * a / POINTS as f64;
            s[p] = 2.0 * b / POINTS as f64;
        }
        Some(LugProfile { c0, c, s })
    }

    /// `(c_p, s_p)`; zero beyond the harmonics the DFT resolves.
    pub fn harmonic(&self, p: usize) -> (f64, f64) {
        if p == 0 || p >= self.c.len() {
            (0.0, 0.0)
        } else {
            (self.c[p], self.s[p])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_is_t_times_the_mean_deviation_and_equal_lugs_only_have_multiples_of_n() {
        let p = LugProfile::new(1500.0, &[0.02; 8], 8, PI / 8.0, 4.0).unwrap();
        assert!((p.c0 - 30.0).abs() < 1e-9, "{}", p.c0);
        for q in 1..40 {
            let (c, s) = p.harmonic(q);
            if q == 8 {
                assert!(c.abs() + s.abs() > 1e-3, "harmonic {q}");
            } else if q % 8 == 0 {
                assert!(c.abs() + s.abs() < 1e-3, "(16, 24, ... fall off fast)");
            } else {
                assert!(c.abs() + s.abs() < 1e-9, "harmonic {q}: {c} {s}");
            }
        }
        let one = LugProfile::new(1500.0, &[0.03, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 8, PI / 8.0, 4.0).unwrap();
        assert!((one.c0 - 1500.0 * 0.03 / 8.0).abs() < 1e-9);
        assert!(one.harmonic(2).0.abs() + one.harmonic(2).1.abs() > 1.0);
        assert!(LugProfile::new(1500.0, &[0.0; 8], 8, 0.0, 4.0).is_none());
    }
}
