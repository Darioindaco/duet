//! Measurement helpers for the tests and examples (never on the audio thread):
//! an FFT, spectral peaks, decay fits and the spectral centroid.

use std::f64::consts::PI;

/// In-place radix-2 FFT (`re`, `im` of a power-of-two length).
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}

/// Magnitude spectrum of a Hann-windowed signal, zero-padded to `n_fft`;
/// bin k is at `k·fs/n_fft`.
pub fn magnitude_spectrum(x: &[f64], n_fft: usize) -> Vec<f64> {
    assert!(n_fft >= x.len() && n_fft.is_power_of_two());
    let n = x.len();
    let mut re = vec![0.0; n_fft];
    let mut im = vec![0.0; n_fft];
    for (i, v) in x.iter().enumerate() {
        re[i] = v * (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos());
    }
    fft(&mut re, &mut im);
    (0..n_fft / 2).map(|k| re[k].hypot(im[k])).collect()
}

/// The frequency (Hz) of the strongest peak within ±`half_width` Hz of `near`,
/// refined by a parabola through the log magnitudes around it.
pub fn peak_near(mag: &[f64], fs: f64, n_fft: usize, near: f64, half_width: f64) -> f64 {
    let bin = |f: f64| (f * n_fft as f64 / fs).round() as usize;
    let (lo, hi) = (bin((near - half_width).max(0.0)), bin(near + half_width).min(mag.len() - 2));
    let k = (lo..=hi).max_by(|a, b| mag[*a].total_cmp(&mag[*b])).unwrap();
    let (a, b, c) = (mag[k - 1].max(1e-300).ln(), mag[k].max(1e-300).ln(), mag[k + 1].max(1e-300).ln());
    let denom = a - 2.0 * b + c;
    let off = if denom.abs() > 1e-300 { 0.5 * (a - c) / denom } else { 0.0 };
    (k as f64 + off) * fs / n_fft as f64
}

/// Least-squares slope of `ln(y)` against `t` (so the decay rate is its negative).
pub fn log_slope(t: &[f64], y: &[f64]) -> f64 {
    let n = t.len() as f64;
    let ly: Vec<f64> = y.iter().map(|v| v.max(1e-300).ln()).collect();
    let (mt, my) = (t.iter().sum::<f64>() / n, ly.iter().sum::<f64>() / n);
    let num: f64 = t.iter().zip(&ly).map(|(t, y)| (t - mt) * (y - my)).sum();
    let den: f64 = t.iter().map(|t| (t - mt).powi(2)).sum();
    num / den
}

/// Spectral centroid (Hz) of a signal.
pub fn spectral_centroid(x: &[f64], fs: f64) -> f64 {
    let n_fft = x.len().next_power_of_two();
    let mag = magnitude_spectrum(x, n_fft);
    let (mut num, mut den) = (0.0, 0.0);
    for (k, m) in mag.iter().enumerate() {
        num += k as f64 * m * m;
        den += m * m;
    }
    if den == 0.0 {
        0.0
    } else {
        num / den * fs / n_fft as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_sine_to_a_fraction_of_a_bin() {
        let fs = 8000.0;
        let x: Vec<f64> = (0..4000).map(|i| (2.0 * PI * 441.3 * i as f64 / fs).sin()).collect();
        let n = 1 << 16;
        let f = peak_near(&magnitude_spectrum(&x, n), fs, n, 440.0, 20.0);
        assert!((f - 441.3).abs() < 0.01, "{f}");
    }
}
