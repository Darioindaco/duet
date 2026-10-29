//! Stage 2: one head, linear, exact modal discretisation (spec §4.9, §6).

use duet_drum::analysis::{log_slope, magnitude_spectrum, peak_near};
use duet_drum::prepare::exact_coefficients;
use duet_drum::{DrumModel, DrumParams, DrumVoice, Head};

fn model(fs: f64, damped: bool) -> (DrumModel, DrumVoice) {
    let mut p = DrumParams::default();
    if !damped {
        p.batter.sigma0 = 0.0;
        p.batter.sigma1 = 0.0;
    }
    let m = DrumModel::prepare_single_head(&p, Head::Batter, fs).unwrap();
    let v = DrumVoice::new(&m);
    (m, v)
}

/// The update matrix of one mode on (x, p), by running the scheme on two unit states.
fn step_matrix(model: &DrumModel, i: usize) -> [[f64; 2]; 2] {
    let (k, d_plus, d_minus, o2) = (model.k, 1.0 + model.k * model_sigma_t(model, i), 1.0 - model.k * model_sigma_t(model, i), model_omega_t2(model, i));
    let step = |x: f64, p: f64| {
        let xh = x + 0.5 * k * p;
        let pn = (d_minus * p - k * o2 * xh) / d_plus;
        (xh + 0.5 * k * pn, pn)
    };
    let (a, b) = (step(1.0, 0.0), step(0.0, 1.0));
    [[a.0, b.0], [a.1, b.1]]
}
fn model_sigma_t(m: &DrumModel, i: usize) -> f64 {
    exact_coefficients(m.modes[i].omega, m.modes[i].sigma, m.k).0
}
fn model_omega_t2(m: &DrumModel, i: usize) -> f64 {
    exact_coefficients(m.modes[i].omega, m.modes[i].sigma, m.k).1
}

#[test]
fn the_coefficients_put_the_scheme_exactly_on_e_to_the_minus_sigma_plus_minus_i_omega_k() {
    // eigenvalues z of the 2×2 update: z² − tr·z + det = 0 must be e^{(−σ ± iΩ)k}
    for fs in [44100.0, 96000.0] {
        let (m, _) = model(fs, true);
        let mut worst = 0f64;
        for i in (0..m.len()).step_by(7) {
            let a = step_matrix(&m, i);
            let (tr, det) = (a[0][0] + a[1][1], a[0][0] * a[1][1] - a[0][1] * a[1][0]);
            let (mo, k) = (&m.modes[i], m.k);
            let want_tr = 2.0 * (-mo.sigma * k).exp() * (mo.omega * k).cos();
            let want_det = (-2.0 * mo.sigma * k).exp();
            worst = worst.max((tr - want_tr).abs()).max((det - want_det).abs());
        }
        assert!(worst < 1e-14, "fs {fs}: {worst}");
    }
}

fn run_and_measure(fs: f64) -> Vec<(f64, f64, f64, f64)> {
    let (m, mut v) = model(fs, true);
    // an impulse off the centre and off the lug axis, so every pair is excited
    let (r_e, th_e) = (0.55, 0.3);
    v.test_impulse(&m, r_e, th_e, 1.0);
    let n = (6.0 * fs) as usize;
    let mic_w = m.projection(0.35, 1.1);
    let mut sig = Vec::with_capacity(n);
    // each of the first ten (m, n)'s: the envelope of its cos member's state
    let firsts: Vec<usize> = m.by_frequency().into_iter().filter(|&i| !m.modes[i].sin).take(10).collect();
    let mut env: Vec<Vec<f64>> = vec![Vec::new(); 10];
    for _ in 0..n {
        v.step_linear(&m, None);
        sig.push(v.displacement(&mic_w));
        for (e, &i) in env.iter_mut().zip(&firsts) {
            let (x, p, mo) = (v.x()[i], v.p()[i], &m.modes[i]);
            // x = A e^{−σt} cos(Ωt+φ): x² + ((p + σx)/Ω)² = (A e^{−σt})²
            e.push((x * x + ((p + mo.sigma * x) / mo.omega).powi(2)).sqrt());
        }
    }
    let n_fft = 1 << 20;
    let mag = magnitude_spectrum(&sig, n_fft);
    let mut out = Vec::new();
    for (b, &i) in firsts.iter().enumerate() {
        let mo = &m.modes[i];
        let f_meas = peak_near(&mag, fs, n_fft, mo.freq(), 3.0);
        // decay over the first 4/σ (the envelope's still far above rounding then)
        let len = ((4.0 / mo.sigma) * fs) as usize;
        let t: Vec<f64> = (0..len).map(|s| s as f64 / fs).collect();
        let sigma_meas = -log_slope(&t, &env[b][..len]);
        out.push((mo.freq(), f_meas, mo.sigma, sigma_meas));
    }
    out
}

#[test]
fn first_ten_modes_ring_at_their_frequency_and_decay_at_their_rate() {
    for fs in [44100.0, 96000.0] {
        for (i, (f, fm, s, sm)) in run_and_measure(fs).into_iter().enumerate() {
            assert!(((fm - f) / f).abs() < 5e-4, "fs {fs} mode {i}: {fm} Hz vs {f} Hz");
            assert!(((sm - s) / s).abs() < 0.02, "fs {fs} mode {i}: σ {sm} vs {s}");
        }
    }
}

#[test]
fn undamped_energy_is_conserved_over_ten_seconds() {
    for fs in [44100.0, 96000.0] {
        let (m, mut v) = model(fs, false);
        v.test_impulse(&m, 0.55, 0.3, 1.0);
        let e0 = v.energy_linear(&m);
        assert!(e0 > 0.0);
        let mut worst = 0f64;
        for s in 0..(10.0 * fs) as usize {
            v.step_linear(&m, None);
            if s % 97 == 0 {
                worst = worst.max(((v.energy_linear(&m) - e0) / e0).abs());
            }
        }
        println!("fs {fs}: worst relative energy drift over 10 s: {worst:.2e}");
        assert!(worst < 1e-12, "fs {fs}: {worst}");
    }
}

#[test]
fn damped_energy_only_falls() {
    let (m, mut v) = model(48000.0, true);
    v.test_impulse(&m, 0.2, 1.0, 1.0);
    let e0 = v.energy_linear(&m);
    let mut last = e0;
    for _ in 0..48000 {
        v.step_linear(&m, None);
        let e = v.energy_linear(&m);
        assert!(e <= last * (1.0 + 1e-12));
        last = e;
    }
    assert!(last < 5e-3 * e0, "a second later the lowest mode (σ ≈ 3/s) is down by e^−6");
}
