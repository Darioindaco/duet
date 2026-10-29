//! Stage 6: lugs, the uneven tension that splits the degenerate pairs (spec §4.5, §6).

use duet_drum::{DrumModel, DrumParams, DrumVoice, Head, Hit};

const FS: f64 = 48000.0;

fn prepare(p: &DrumParams) -> DrumModel {
    DrumModel::prepare(p, FS).unwrap()
}

fn with_lugs(dev: [f64; 8]) -> DrumParams {
    let mut p = DrumParams::default();
    p.batter.lug_deviations = dev.to_vec();
    p
}

/// The pair's two members: (Ω_a, Ω_b) with Ω_a ≥ Ω_b.
fn pair(m: &DrumModel, head: Head, mm: usize, nn: usize) -> Option<(f64, f64)> {
    let a = m.find_mode(head, mm, nn, false)?;
    let b = m.find_mode(head, mm, nn, true)?;
    Some((m.modes[a].omega, m.modes[b].omega))
}

fn batter_pairs(m: &DrumModel) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = m.modes.iter().filter(|x| x.head == Head::Batter && x.m >= 1 && x.block == duet_drum::Block::Pair).map(|x| (x.m, x.n)).collect();
    v.dedup();
    v
}

#[test]
fn equal_lugs_only_split_the_pairs_whose_2m_is_a_multiple_of_the_lug_count() {
    // all eight lugs 2 % tighter: that is a uniform +2 % in tension (c_0 = 30 N/m) and, because the
    // lugs are discrete, a tiny ripple at harmonics of 8: only m = 4, 8, ... split
    let m = prepare(&with_lugs([0.02; 8]));
    let mut reference = DrumParams::default();
    reference.batter.tension = 1500.0 * 1.02;
    let r = prepare(&reference);
    assert_eq!(m.len(), r.len());
    let mut n_split = 0;
    for (mm, nn) in batter_pairs(&m) {
        let (a, b) = pair(&m, Head::Batter, mm, nn).unwrap();
        let (ra, _) = pair(&r, Head::Batter, mm, nn).unwrap();
        let split = (a - b) / a;
        if (2 * mm) % 8 == 0 {
            // (the ripple at harmonic 2m falls off fast with m: m = 4 splits visibly, m = 8, 12 hardly)
            n_split += (split > 1e-7) as usize;
            assert!(split >= 0.0, "({mm},{nn})");
            if mm == 4 {
                assert!(split > 1e-7, "(4,{nn}) should split: {split:e}");
            }
        } else {
            assert!(split.abs() < 1e-9, "({mm},{nn}) must not split: {split:e}");
            // and the mean shift is exactly the tension's 2 %
            assert!(((a - ra) / ra).abs() < 1e-9, "({mm},{nn}): {a} vs {ra}");
        }
    }
    assert!(n_split >= 3, "m = 4 for the first few n: {n_split}");
    // the axisymmetric modes feel the mean shift too
    for i in 0..m.n_axisym() {
        assert!(((m.modes[i].omega - r.modes[i].omega) / r.modes[i].omega).abs() < 1e-9, "block A mode {i}");
    }
}

#[test]
fn one_detuned_lug_splits_the_low_pairs_and_keeps_their_mean() {
    let m = prepare(&with_lugs([0.03, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]));
    // the reference: the mean shift c_0 = T·0.03/8 only
    let mut reference = DrumParams::default();
    reference.batter.tension = 1500.0 * (1.0 + 0.03 / 8.0);
    let r = prepare(&reference);
    for (mm, nn) in [(1, 1), (1, 2), (2, 1), (3, 1)] {
        let (a, b) = pair(&m, Head::Batter, mm, nn).unwrap();
        println!("({mm},{nn}): split {:.4} Hz", (a - b) / (2.0 * std::f64::consts::PI));
        assert!(a > b, "({mm},{nn}) splits");
        // first-order perturbation of a symmetric 2×2 with zero trace: the mean of Ω² is untouched
        let (ra, _) = pair(&r, Head::Batter, mm, nn).unwrap();
        assert!((0.5 * (a * a + b * b) - ra * ra).abs() < 1e-9 * ra * ra, "({mm},{nn}) mean of Ω²");
    }
    let (a, b) = pair(&m, Head::Batter, 1, 1).unwrap();
    assert!((a - b) / a > 1e-4);
    // the (1,1) pair's principal axes point at the detuned lug (θ_lug0 = π/8): φ = π/8
    let i = m.find_mode(Head::Batter, 1, 1, false).unwrap();
    assert!((m.modes[i].phi - std::f64::consts::PI / 8.0).abs() < 1e-3, "{}", m.modes[i].phi);
}

#[test]
fn the_pairs_rotate_but_keep_their_weight_at_a_point() {
    // (e_a² + e_b²) at any point is the unsplit pair's (e_c² + e_s²): a rotation
    let m = prepare(&with_lugs([0.05, 0.0, -0.02, 0.0, 0.0, 0.03, 0.0, 0.0]));
    let mut reference = DrumParams::default();
    reference.batter.tension = 1500.0;
    let r = prepare(&reference);
    for (r_frac, th) in [(0.3, 0.4), (0.7, 2.0), (0.9, 5.0)] {
        let (w, w0) = (m.projection(r_frac, th), r.projection(r_frac, th));
        for (mm, nn) in batter_pairs(&m) {
            let sum = |m: &DrumModel, w: &[f64]| {
                let a = m.find_mode(Head::Batter, mm, nn, false).unwrap();
                let b = m.find_mode(Head::Batter, mm, nn, true).unwrap();
                w[a] * w[a] + w[b] * w[b]
            };
            let (s, s0) = (sum(&m, &w), sum(&r, &w0));
            assert!((s - s0).abs() <= 1e-9 * s0.max(1e-12), "({mm},{nn}) at {r_frac},{th}: {s} vs {s0}");
        }
    }
}

#[test]
fn undamped_energy_is_conserved_with_lugs() {
    let mut p = with_lugs([0.04, 0.0, -0.03, 0.0, 0.02, 0.0, 0.0, -0.01]);
    for h in [Head::Batter, Head::Resonant] {
        p.head_mut(h).sigma0 = 0.0;
        p.head_mut(h).sigma1 = 0.0;
    }
    p.resonant.lug_deviations = vec![0.0, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    p.radiation_on = false;
    let m = prepare(&p);
    let mut v = DrumVoice::new(&m);
    v.trigger(&m, Hit { velocity: 0.9, radius: 0.6, angle: 1.0, sample_offset: 0 });
    let e0 = v.energy(&m);
    let mut worst = 0f64;
    for _ in 0..(1.5 * FS) as usize {
        v.step(&m);
        worst = worst.max(((v.energy(&m) + v.stick_energy_removed() - e0) / e0).abs());
    }
    println!("worst relative energy drift with lugs: {worst:.2e}");
    assert!(worst < 1e-12, "{worst}");
}

#[test]
fn no_deviation_is_exactly_the_old_model_and_a_wrong_lug_count_is_refused() {
    let a = prepare(&DrumParams::default());
    let mut p = DrumParams::default();
    p.batter.lug_deviations = vec![];
    let b = prepare(&p);
    assert_eq!(a.len(), b.len());
    for i in 0..a.len() {
        assert_eq!(a.modes[i].omega, b.modes[i].omega);
        assert_eq!(a.modes[i].phi, 0.0);
    }
    p.batter.lug_deviations = vec![0.01; 5];
    assert!(DrumModel::prepare(&p, FS).is_err());
}
