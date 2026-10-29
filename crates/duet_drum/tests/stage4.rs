//! Stage 4: tension modulation (spec §4.8a, §6).

use duet_drum::analysis::mode_frequency_track;
use duet_drum::{DrumModel, DrumParams, DrumVoice, Head, Hit};

fn model(fs: f64, p: DrumParams) -> DrumModel {
    DrumModel::prepare_single_head(&p, Head::Batter, fs).unwrap()
}

fn hit(velocity: f32) -> Hit {
    Hit { velocity, radius: 0.5, angle: 0.4, sample_offset: 0 }
}

/// Cents above the linear frequency of `mode`, in windows centred at `times`.
fn glide(m: &DrumModel, vel: f32, mode: usize, times: &[f64]) -> Vec<f64> {
    let mut v = DrumVoice::new(m);
    v.trigger(m, hit(vel));
    let n = (0.7 * m.fs) as usize;
    let (mut x, mut p) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..n {
        v.step(m);
        x.push(v.x()[mode]);
        p.push(v.p()[mode]);
    }
    let mo = &m.modes[mode];
    let tr = mode_frequency_track(&x, &p, mo.omega, mo.sigma, m.fs, (0.04 * m.fs) as usize);
    times
        .iter()
        .map(|t| {
            let f = tr.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs())).unwrap().1;
            1200.0 * (f / mo.freq()).log2()
        })
        .collect()
}

const TIMES: [f64; 4] = [0.035, 0.1, 0.2, 0.5];

#[test]
fn a_hard_hit_starts_sharp_and_glides_down() {
    let m = model(48000.0, DrumParams::default());
    for (mm, nn) in [(0usize, 1usize), (1, 1)] {
        let mode = m.find_mode(Head::Batter, mm, nn, false).unwrap();
        let g = glide(&m, 1.0, mode, &TIMES);
        println!("mode {mode}: {g:.1?} cents at 35 / 100 / 200 / 500 ms");
        assert!(g[0] > 20.0, "measurably sharp: {g:?}");
        assert!(g.windows(2).all(|w| w[1] < w[0]), "and falling: {g:?}");
        assert!(g[2] < 0.6 * g[0] && g[3] < 0.2 * g[0], "{g:?}");
    }
}

#[test]
fn the_glide_deepens_with_velocity_and_is_nothing_for_a_soft_hit() {
    let m = model(48000.0, DrumParams::default());
    let depth: Vec<f64> = [0.1f32, 0.4, 0.7, 1.0].iter().map(|v| glide(&m, *v, m.find_mode(Head::Batter, 0, 1, false).unwrap(), &[0.035])[0]).collect();
    println!("glide at 35 ms, vel 0.1 / 0.4 / 0.7 / 1.0: {depth:.1?} cents");
    assert!(depth.windows(2).all(|w| w[1] > w[0]), "{depth:?}");
    assert!(depth[0].abs() < 1.0, "vel 0.1: {}", depth[0]);
}

#[test]
fn the_glide_does_not_depend_on_the_sample_rate() {
    let g48 = glide(&model(48000.0, DrumParams::default()), 1.0, 0, &[0.035, 0.2]);
    let g192 = glide(&model(192000.0, DrumParams::default()), 1.0, 0, &[0.035, 0.2]);
    println!("vel 1.0 at 48 kHz {g48:.1?}, at 192 kHz {g192:.1?}");
    for i in 0..2 {
        assert!((g48[i] - g192[i]).abs() < 0.05 * g192[i], "{g48:?} vs {g192:?}");
    }
}

#[test]
fn nonlinearity_scale_zero_switches_it_off() {
    let mut p = DrumParams::default();
    p.nonlinearity_scale = 0.0;
    let g = glide(&model(48000.0, p), 1.0, 0, &[0.035]);
    assert!(g[0].abs() < 0.5, "{g:?}");
}

#[test]
fn undamped_energy_is_still_conserved() {
    let mut p = DrumParams::default();
    p.batter.sigma0 = 0.0;
    p.batter.sigma1 = 0.0;
    let m = model(48000.0, p);
    let mut worst = 0f64;
    for vel in [0.4f32, 0.7, 1.0] {
        let mut v = DrumVoice::new(&m);
        v.trigger(&m, hit(vel));
        let e0 = v.energy(&m);
        for _ in 0..(2.0 * m.fs) as usize {
            v.step(&m);
            worst = worst.max(((v.energy(&m) + v.stick_energy_removed() - e0) / e0).abs());
        }
    }
    println!("worst relative energy drift with tension modulation: {worst:.2e}");
    assert!(worst < 1e-12, "{worst}");
}
