//! Stage 5: two heads, cavity, air loading, radiation damping (spec §4.3, §4.4, §4.6, §4.7, §6).

use duet_drum::{Block, DrumModel, DrumParams, DrumVoice, Head, Hit, PrepareOptions};

const FS: f64 = 48000.0;

fn full(p: &DrumParams) -> DrumModel {
    DrumModel::prepare(p, FS).unwrap()
}

fn pairs(m: &DrumModel, head: Head) -> Vec<(usize, usize, bool, f64)> {
    m.modes.iter().filter(|x| x.block == Block::Pair && x.head == head).map(|x| (x.m, x.n, x.sin, x.omega)).collect()
}

#[test]
fn default_model_prepares_cleanly() {
    let t = std::time::Instant::now();
    let m = full(&DrumParams::default());
    let took = t.elapsed();
    let nb = m.modes.iter().filter(|x| x.head == Head::Batter).count();
    let nr = m.modes.iter().filter(|x| x.head == Head::Resonant).count();
    println!("{} modes ({} axisymmetric; batter {nb}, resonant {nr}), prepared in {took:.1?}", m.len(), m.n_axisym());
    assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    for x in &m.modes {
        assert!((x.participation[0] + x.participation[1] - 1.0).abs() < 1e-12, "participation sums to 1");
        assert!(x.sigma < x.omega, "underdamped");
        assert!(x.freq() <= 2500.0 + 1e-9 && x.freq() > 50.0);
    }
    assert!(nb <= 1200 && nr <= 1200);
}

#[test]
fn air_loading_puts_the_one_one_pair_where_tension_for_pitch_says() {
    // T = tension_for_pitch(f): the head's lowest (1,1) pair, air-loaded, lands on f
    for head in [Head::Batter, Head::Resonant] {
        let mut p = DrumParams::default();
        let t = p.tension_for_pitch(head, 150.0);
        p.head_mut(head).tension = t;
        let m = full(&p);
        let i = m.find_mode(head, 1, 1, false).unwrap();
        assert!((m.modes[i].freq() - 150.0).abs() < 1e-9, "{head:?}: {} Hz (tension {t})", m.modes[i].freq());
    }
}

#[test]
fn the_pairs_do_not_feel_the_cavity() {
    let a = full(&DrumParams::default());
    for scale in [0.0, 10.0] {
        let mut p = DrumParams::default();
        p.cavity_scale = scale;
        let b = full(&p);
        for head in [Head::Batter, Head::Resonant] {
            assert_eq!(pairs(&a, head), pairs(&b, head), "{head:?}, cavity ×{scale}");
        }
        // ...but the axisymmetric ones do
        let fa: Vec<f64> = a.modes[..a.n_axisym()].iter().map(|x| x.freq()).collect();
        let fb: Vec<f64> = b.modes[..b.n_axisym()].iter().map(|x| x.freq()).collect();
        assert!((fa[0] - fb[0]).abs() > 1.0 || (fa[1] - fb[1]).abs() > 1.0, "{fa:?} vs {fb:?}");
    }
}

#[test]
fn the_resonant_tension_moves_the_axisymmetric_modes_and_not_the_batters_pairs() {
    let a = full(&DrumParams::default());
    let mut p = DrumParams::default();
    p.resonant.tension = 1300.0;
    let b = full(&p);
    assert_eq!(pairs(&a, Head::Batter), pairs(&b, Head::Batter), "the batter's pairs don't know");
    assert_ne!(pairs(&a, Head::Resonant), pairs(&b, Head::Resonant));
    let moved = (0..a.n_axisym().min(b.n_axisym())).filter(|&i| (a.modes[i].freq() - b.modes[i].freq()).abs() > 0.5).count();
    assert!(moved >= 4, "the coupled m = 0 modes shift: {moved}");
}

#[test]
fn radiation_damps_the_axisymmetric_modes_only() {
    let m = full(&DrumParams::default());
    let p = DrumParams::default();
    for x in &m.modes {
        match x.block {
            Block::Pair => {
                assert_eq!(x.sigma_rad, 0.0);
                let h = p.head(x.head);
                assert!((x.sigma - (h.sigma0 + h.sigma1 * x.omega * x.omega)).abs() < 1e-9 * x.sigma);
            }
            Block::Axisym => assert!(x.sigma_rad >= 0.0 && x.sigma > x.sigma_int - 1e-12),
        }
    }
    // breathing (both heads out together, a net volume change) and sloshing (net volume ~ 0):
    // the lowest two axisymmetric modes
    let (slosh, breathe) = if m.modes[0].vol_weight.abs() < m.modes[1].vol_weight.abs() { (&m.modes[0], &m.modes[1]) } else { (&m.modes[1], &m.modes[0]) };
    println!(
        "(0,1) pair: sloshing {:.1} Hz, ã = {:.3}, T60 {:.3} s; breathing {:.1} Hz, ã = {:.3}, T60 {:.3} s",
        slosh.freq(), slosh.vol_weight, slosh.t60(), breathe.freq(), breathe.vol_weight, breathe.t60()
    );
    assert!(breathe.freq() > slosh.freq(), "the cavity stiffens the breathing one");
    assert!(breathe.sigma_rad > 5.0 * slosh.sigma_rad, "and it radiates: {} vs {}", breathe.sigma_rad, slosh.sigma_rad);
    assert!(breathe.t60() < 0.5 * slosh.t60());
    // without radiation, neither is damped more than the internal damping
    let mut q = DrumParams::default();
    q.radiation_on = false;
    assert!(full(&q).modes.iter().all(|x| x.sigma_rad == 0.0));
    // single head, in vacuo, no cavity: the old model, mode for mode
    let s = DrumModel::prepare_with(&p, FS, &PrepareOptions { resonant: false, air_loading: false, cavity: false }).unwrap();
    let t = DrumModel::prepare_single_head(&p, Head::Batter, FS).unwrap();
    assert_eq!(s.len(), t.len());
}

#[test]
fn undamped_energy_is_conserved_with_both_heads_the_cavity_and_the_stick() {
    let mut p = DrumParams::default();
    for h in [Head::Batter, Head::Resonant] {
        p.head_mut(h).sigma0 = 0.0;
        p.head_mut(h).sigma1 = 0.0;
    }
    p.radiation_on = false;
    let m = full(&p);
    let mut worst = 0f64;
    for (vel, r, th) in [(1.0f32, 0.0, 0.0), (0.7, 0.5, 1.0), (0.4, 0.85, 2.5)] {
        let mut v = DrumVoice::new(&m);
        v.trigger(&m, Hit { velocity: vel, radius: r, angle: th, sample_offset: 0 });
        let e0 = v.energy(&m);
        for _ in 0..(1.5 * FS) as usize {
            v.step(&m);
            worst = worst.max(((v.energy(&m) + v.stick_energy_removed() - e0) / e0).abs());
        }
    }
    println!("worst relative energy drift, two heads + cavity + tension modulation + stick: {worst:.2e}");
    assert!(worst < 1e-12, "{worst}");
}

#[test]
fn the_resonant_head_answers_and_nothing_blows_up() {
    // a hit on the batter sets the resonant head moving through the cavity (the axisymmetric
    // modes): its energy share becomes non-zero; and 2,000 random hits stay finite
    let m = full(&DrumParams::default());
    let mut v = DrumVoice::new(&m);
    v.trigger(&m, Hit { velocity: 0.8, radius: 0.1, angle: 0.0, sample_offset: 0 });
    let mut max_share = 0f64;
    for _ in 0..4800 {
        v.step(&m);
        let (mut eb, mut er) = (0.0, 0.0);
        for (i, x) in m.modes.iter().enumerate().take(m.n_axisym()) {
            let e = 0.5 * v.p()[i].powi(2);
            eb += e * x.participation[0];
            er += e * x.participation[1];
        }
        if eb + er > 0.0 {
            max_share = max_share.max(er / (eb + er));
        }
    }
    println!("largest share of the axisymmetric kinetic energy in the resonant head: {:.0} %", max_share * 100.0);
    assert!(max_share > 0.1, "{max_share}");
    let mut rng = 0x2545F4914F6CDD1Du64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut v = DrumVoice::new(&m);
    for _ in 0..2000 {
        v.trigger(&m, Hit { velocity: next() as f32, radius: next() * 0.95, angle: next() * 6.283, sample_offset: 0 });
        for _ in 0..(next() * 400.0) as usize + 50 {
            v.step(&m);
        }
        assert!(v.energy(&m).is_finite() && v.force().is_finite());
    }
}
