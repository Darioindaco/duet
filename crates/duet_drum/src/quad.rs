//! Gauss–Legendre quadrature (nodes by Newton's method on P_n).

/// Nodes and weights on [-1, 1], ascending.
pub fn gauss_legendre(n: usize) -> (Vec<f64>, Vec<f64>) {
    assert!(n >= 1);
    let mut x = vec![0.0; n];
    let mut w = vec![0.0; n];
    for i in 0..(n + 1) / 2 {
        let mut z = (std::f64::consts::PI * (i as f64 + 0.75) / (n as f64 + 0.5)).cos();
        let mut pp = 1.0;
        for _ in 0..100 {
            // P_n(z) by the three-term recurrence
            let (mut p1, mut p2) = (1.0, 0.0);
            for j in 0..n {
                let p3 = p2;
                p2 = p1;
                p1 = ((2 * j + 1) as f64 * z * p2 - j as f64 * p3) / (j + 1) as f64;
            }
            pp = n as f64 * (z * p1 - p2) / (z * z - 1.0);
            let z1 = z;
            z = z1 - p1 / pp;
            if (z - z1).abs() < 1e-15 {
                break;
            }
        }
        x[i] = -z;
        x[n - 1 - i] = z;
        w[i] = 2.0 / ((1.0 - z * z) * pp * pp);
        w[n - 1 - i] = w[i];
    }
    (x, w)
}

/// Nodes and weights on [a, b].
pub fn gauss_legendre_on(n: usize, a: f64, b: f64) -> (Vec<f64>, Vec<f64>) {
    let (x, w) = gauss_legendre(n);
    let (h, c) = (0.5 * (b - a), 0.5 * (b + a));
    (x.iter().map(|x| c + h * x).collect(), w.iter().map(|w| h * w).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrates_polynomials_exactly() {
        let (x, w) = gauss_legendre_on(10, 0.0, 2.0);
        // degree 19 is exact with 10 points
        let got: f64 = x.iter().zip(&w).map(|(x, w)| w * x.powi(19)).sum();
        assert!((got - 2f64.powi(20) / 20.0).abs() < 1e-8 * 2f64.powi(20) / 20.0);
        assert!((w.iter().sum::<f64>() - 2.0).abs() < 1e-14);
    }
}
