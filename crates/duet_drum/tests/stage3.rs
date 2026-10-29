//! Stage 3: the stick's contact, integrated with the SAV scheme (spec §4.8b, §4.9, §4.11, §6).

use duet_drum::analysis::spectral_centroid;
use duet_drum::{DrumModel, DrumParams, DrumVoice, Head, Hit};

fn model(fs: f64, damped: bool) -> DrumModel {
    let mut p = DrumParams::default();
    if !damped {
        p.batter.sigma0 = 0.0;
        p.batter.sigma1 = 0.0;
    }
    DrumModel::prepare_single_head(&p, Head::Batter, fs).unwrap()
}

fn hit(velocity: f32, radius: f64, angle: f64) -> Hit {
    Hit { velocity, radius, angle, sample_offset: 0 }
}

#[test]
fn undamped_energy_including_the_stick_is_conserved() {
    let fs = 48000.0;
    let m = model(fs, false);
    let mut worst = 0f64;
    for (vel, r, th) in [(0.1, 0.0, 0.0), (0.4, 0.3, 0.7), (0.7, 0.6, 1.9), (1.0, 0.9, 0.3), (1.0, 0.0, 0.0), (0.05, 0.5, 2.0)] {
        let mut v = DrumVoice::new(&m);
        v.trigger(&m, hit(vel, r, th));
        let e0 = v.energy(&m);
        assert!(e0 > 0.0);
        let mut left = false;
        for _ in 0..(2.0 * fs) as usize {
            v.step(&m);
            // the stick's own energy leaves with it; everything else must add up
            let drift = ((v.energy(&m) + v.stick_energy_removed() - e0) / e0).abs();
            worst = worst.max(drift);
            left |= !v.stick_active();
        }
        assert!(left, "the stick bounces off (vel {vel}, r {r})");
        assert!(v.force() == 0.0);
    }
    println!("worst relative energy drift (stick included), 6 hits × 2 s: {worst:.2e}");
    assert!(worst < 1e-12, "{worst}");
}

#[test]
fn ten_thousand_random_hits_stay_finite() {
    // 100 voices in 4 threads, 100 hits each, thrown at a head that is still ringing
    let fs = 44100.0;
    let m = model(fs, true);
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let m = m.clone();
            std::thread::spawn(move || {
                let mut rng = 0x9E3779B97F4A7C15u64 ^ (t as u64 * 0x1234567);
                let mut next = move || {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng >> 11) as f64 / (1u64 << 53) as f64
                };
                let mut peak = 0f64;
                for _ in 0..25 {
                    let mut v = DrumVoice::new(&m);
                    for _ in 0..100 {
                        v.trigger(&m, hit(next() as f32, next() * 0.95, next() * 6.283));
                        for _ in 0..(next() * 600.0) as usize + 100 {
                            v.step(&m);
                        }
                        let e = v.energy(&m);
                        assert!(e.is_finite() && v.force().is_finite() && v.psi().is_finite(), "non-finite state");
                        peak = peak.max(e);
                    }
                    assert!(v.x().iter().chain(v.p()).all(|x| x.is_finite()));
                }
                peak
            })
        })
        .collect();
    let peak = handles.into_iter().map(|h| h.join().unwrap()).fold(0f64, f64::max);
    println!("10,000 hits: all finite; the largest energy seen {peak:.3} J");
    // 100 hits of at most 6 m/s on 20 g (0.36 J each) are all that goes in; the head has
    // no way to hold more (it is damped, and the stick takes energy away when it leaves)
    assert!(peak < 100.0 * 0.36);
}

// (stage 3 could not show this: with the head's compliance dominating, the system was nearly
// linear. Tension modulation, stage 4, brings it.)
#[test]
fn harder_hits_are_brighter() {
    // the centroid of the head's acceleration over 0.25 s, averaged over a few strike points
    // and angles (one point alone depends too much on which modes it happens to excite)
    let fs = 48000.0;
    let m = model(fs, true);
    let mut centroids = Vec::new();
    for vel in [0.1f32, 0.4, 0.7, 1.0] {
        let mut sum = 0.0;
        let spots = [(0.25, 0.4), (0.5, 1.7), (0.7, 3.1), (0.4, 4.6)];
        for (r, th) in spots {
            let mut v = DrumVoice::new(&m);
            v.trigger(&m, hit(vel, r, th));
            let mut a = Vec::new();
            for _ in 0..(0.25 * fs) as usize {
                v.step(&m);
                a.push(v.acceleration_at(&m, 0.4, 2.0));
            }
            sum += spectral_centroid(&a, fs);
        }
        centroids.push(sum / spots.len() as f64);
    }
    println!("spectral centroid of the head's acceleration, vel 0.1 / 0.4 / 0.7 / 1.0: {centroids:.0?} Hz");
    assert!(centroids.windows(2).all(|w| w[1] > w[0]), "{centroids:?}");
}

/// The contact's peak force (N) and first-to-last contact time (ms), for a stiffness and a rate.
fn contact(ks: f64, fs: f64) -> (f64, f64) {
    let mut p = DrumParams::default();
    p.stick.stiffness = ks;
    let m = DrumModel::prepare_single_head(&p, Head::Batter, fs).unwrap();
    let mut v = DrumVoice::new(&m);
    v.trigger(&m, hit(0.7, 0.3, 0.3));
    let (mut first, mut last, mut peak) = (None, 0usize, 0f64);
    for s in 0..(0.05 * fs) as usize {
        v.step(&m);
        if v.force() > 0.0 {
            first.get_or_insert(s);
            last = s;
            peak = peak.max(v.force());
        }
    }
    (peak, (last - first.expect("the stick touches the head")) as f64 / fs * 1e3)
}

#[test]
fn a_soft_contact_converges_with_the_sample_rate() {
    // K_s = 1e6 (the default): one smooth pulse that doesn't depend on the rate (the spec's
    // K_s = 1e8 chatters, and its force changes by 2.5× from 48 to 192 kHz: see the stage-3 report)
    assert_eq!(DrumParams::default().stick.stiffness, 1e6);
    let (p48, t48) = contact(1e6, 48000.0);
    let (p96, t96) = contact(1e6, 96000.0);
    let (p192, t192) = contact(1e6, 192000.0);
    println!("K_s = 1e6: peak force {p48:.2} / {p96:.2} / {p192:.2} N, contact {t48:.2} / {t96:.2} / {t192:.2} ms at 48 / 96 / 192 kHz");
    assert!((p48 - p192).abs() < 0.01 * p192 && (p96 - p192).abs() < 0.01 * p192);
    assert!((t48 - t192).abs() < 0.1 * t192);
}

#[test]
fn the_spec_contact_is_finite_whatever_it_does() {
    // (K_s = 1e8, the spec's value: ill-resolved but it must still not blow up)
    for vel in [0.1f32, 0.4, 0.7, 1.0] {
        for r in [0.0, 0.3, 0.6, 0.9] {
            let mut p = DrumParams::default();
            p.stick.stiffness = 1e8;
            let m = DrumModel::prepare_single_head(&p, Head::Batter, 48000.0).unwrap();
            let mut v = DrumVoice::new(&m);
            v.trigger(&m, hit(vel, r, 0.3));
            let mut peak = 0f64;
            for _ in 0..2400 {
                v.step(&m);
                peak = peak.max(v.force());
            }
            assert!(peak > 0.0 && peak.is_finite(), "vel {vel} r {r}");
        }
    }
}
