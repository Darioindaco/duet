//! Bessel functions of the first kind, integer order, and their zeros.
//!
//! * `J_m(x)` by Miller's backward recurrence, normalised with
//!   `J_0 + 2 Σ J_2k = 1`. Accurate to a few 1e-16 absolute for x up to a few
//!   hundred, any m (the recurrence runs downward, where it is stable).
//! * Zeros: the zeros of `J_0` start from McMahon's asymptotic expansion; the
//!   zeros of `J_m` then sit in the brackets the zeros of `J_{m-1}` give
//!   (`j_{m-1,n} < j_{m,n} < j_{m-1,n+1}`), so none can be missed. Each is
//!   refined by a safeguarded Newton iteration inside its bracket.
//!
//! Prepare-time only: allocates.

use std::f64::consts::PI;

/// `J_0(x) ..= J_mmax(x)` for `x >= 0`.
pub fn bessel_j_upto(x: f64, mmax: usize) -> Vec<f64> {
    assert!(x >= 0.0 && x.is_finite(), "bessel: x must be finite and >= 0");
    let mut out = vec![0.0; mmax + 1];
    if x == 0.0 {
        out[0] = 1.0;
        return out;
    }
    // start well past both x and mmax: beyond x the J's fall off faster than
    // exponentially, so whatever we start with is gone by the time we're down
    let big = mmax.max(x.ceil() as usize);
    let mut n = big + 25 + (15.0 * (big as f64).cbrt()) as usize;
    n += n % 2;
    let mut v = vec![0.0f64; n + 2];
    v[n] = 1e-30;
    for k in (1..=n).rev() {
        v[k - 1] = 2.0 * k as f64 / x * v[k] - v[k + 1];
        if v[k - 1].abs() > 1e250 {
            for e in &mut v[k - 1..] {
                *e *= 1e-250;
            }
        }
    }
    let mut sum = v[0];
    for k in (2..=n).step_by(2) {
        sum += 2.0 * v[k];
    }
    for (o, e) in out.iter_mut().zip(&v) {
        *o = e / sum;
    }
    out
}

/// `J_m(x)`.
pub fn bessel_j(m: usize, x: f64) -> f64 {
    bessel_j_upto(x, m)[m]
}

/// `(J_m(x), J_m'(x))`, with `J_m' = (J_{m-1} - J_{m+1})/2` and `J_0' = -J_1`.
pub fn bessel_j_dj(m: usize, x: f64) -> (f64, f64) {
    let v = bessel_j_upto(x, m + 1);
    let d = if m == 0 { -v[1] } else { 0.5 * (v[m - 1] - v[m + 1]) };
    (v[m], d)
}

/// The modified Bessel function `I_0(x)` (power series; x ≥ 0 up to a few hundred).
pub fn bessel_i0(x: f64) -> f64 {
    let q = 0.25 * x * x;
    let (mut term, mut sum) = (1.0, 1.0);
    for k in 1..500 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-17 * sum {
            break;
        }
    }
    sum
}

/// The one zero of `J_m` inside `(a, b)` (`J_m` must change sign there).
fn zero_in(m: usize, a: f64, b: f64) -> f64 {
    let (mut lo, mut hi) = (a, b);
    let flo = bessel_j(m, lo);
    debug_assert!(flo * bessel_j(m, hi) <= 0.0, "no sign change for m={m} in ({a}, {b})");
    let mut x = 0.5 * (lo + hi);
    for _ in 0..200 {
        let (f, df) = bessel_j_dj(m, x);
        if f == 0.0 {
            return x;
        }
        if (f > 0.0) == (flo > 0.0) {
            lo = x;
        } else {
            hi = x;
        }
        let mut xn = x - f / df;
        if !(xn > lo && xn < hi) {
            xn = 0.5 * (lo + hi); // Newton left the bracket: bisect
        }
        if (xn - x).abs() <= 1e-15 * xn {
            return xn;
        }
        x = xn;
    }
    x
}

/// The n-th (1-based) zero of `J_0`: McMahon's expansion, then Newton.
fn zero0(n: usize) -> f64 {
    // j_{0,n} ~ b + 1/(8b) - 31/(384 b^3), b = (n - 1/4) pi
    let beta = (n as f64 - 0.25) * PI;
    let guess = beta + 1.0 / (8.0 * beta) - 31.0 / (384.0 * beta.powi(3));
    zero_in(0, (guess - 0.3).max(1e-3), guess + 0.3)
}

/// The zeros of `J_0 ..= J_m_max`, at least `n_max` of each.
#[derive(Clone, Debug)]
pub struct ZeroTable {
    rows: Vec<Vec<f64>>,
}

impl ZeroTable {
    pub fn new(m_max: usize, n_max: usize) -> Self {
        // each order has one zero fewer than the one before (its brackets)
        let n0 = n_max + m_max + 1;
        let mut rows: Vec<Vec<f64>> = Vec::with_capacity(m_max + 1);
        rows.push((1..=n0).map(zero0).collect());
        for m in 1..=m_max {
            let prev = &rows[m - 1];
            let row: Vec<f64> = (0..prev.len() - 1).map(|i| zero_in(m, prev[i], prev[i + 1])).collect();
            rows.push(row);
        }
        ZeroTable { rows }
    }

    /// `j_{m,n}`, n counted from 1.
    pub fn zero(&self, m: usize, n: usize) -> f64 {
        self.rows[m][n - 1]
    }

    /// All the zeros of `J_m` the table holds, ascending.
    pub fn row(&self, m: usize) -> &[f64] {
        &self.rows[m]
    }

    pub fn m_max(&self) -> usize {
        self.rows.len() - 1
    }
}

/// `j_{m,n}`, the n-th positive zero of `J_m` (n from 1).
pub fn bessel_zero(m: usize, n: usize) -> f64 {
    ZeroTable::new(m, n).zero(m, n)
}

/// The first `count` positive zeros of `J_m`.
pub fn bessel_zeros(m: usize, count: usize) -> Vec<f64> {
    ZeroTable::new(m, count).row(m)[..count].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        assert!((bessel_j(0, 1.0) - 0.7651976865579666).abs() < 1e-15);
        assert!((bessel_j(1, 1.0) - 0.4400505857449335).abs() < 1e-15);
        assert!((bessel_j(5, 10.0) - -0.2340615281867936).abs() < 1e-15);
        assert_eq!(bessel_j(3, 0.0), 0.0);
        assert_eq!(bessel_j(0, 0.0), 1.0);
    }

    #[test]
    fn i0_known_values() {
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-16);
        assert!((bessel_i0(4.0) - 11.301921952136330).abs() < 1e-13);
        assert!((bessel_i0(1.0) - 1.2660658777520082).abs() < 1e-15);
    }

    #[test]
    fn first_zero_of_j0() {
        assert!((bessel_zero(0, 1) - 2.404825557695773).abs() < 1e-12);
    }
}
