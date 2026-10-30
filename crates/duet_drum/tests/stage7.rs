//! Stage 7: the outputs, their gains and the block interface (spec §4.10, §4.11, §6).

use duet_drum::{DrumModel, DrumParams, DrumVoice, Hit};

fn model(fs: f64) -> DrumModel {
    DrumModel::prepare(&DrumParams::default(), fs).unwrap()
}

fn hit(velocity: f32, radius: f64, angle: f64) -> Hit {
    Hit { velocity, radius, angle, sample_offset: 0 }
}

/// Renders `total` samples in blocks of `block`; `hits` are (global sample, hit).
fn render(m: &DrumModel, hits: &[(usize, Hit)], total: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut v = DrumVoice::new(m);
    let (mut close, mut far) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut c, mut f) = (vec![0f32; block], vec![0f32; block]);
    let mut t = 0;
    while t < total {
        let len = block.min(total - t);
        for (at, h) in hits {
            if *at >= t && *at < t + len {
                v.trigger(m, Hit { sample_offset: (*at - t) as u32, ..*h });
            }
        }
        v.process(m, &mut c[..len], &mut f[..len]);
        close.extend_from_slice(&c[..len]);
        far.extend_from_slice(&f[..len]);
        t += len;
    }
    (close, far)
}

fn peak_db(x: &[f32]) -> f64 {
    20.0 * (x.iter().fold(0f32, |a, v| a.max(v.abs())) as f64).log10()
}

#[test]
fn a_full_velocity_centre_hit_peaks_near_minus_six_dbfs_at_any_rate() {
    for fs in [44100.0, 48000.0, 96000.0] {
        let m = model(fs);
        let (c, f) = render(&m, &[(0, hit(1.0, 0.0, 0.0))], (1.5 * fs) as usize, 512);
        println!("fs {fs}: close {:.2} dBFS, far {:.2} dBFS", peak_db(&c), peak_db(&f));
        assert!((peak_db(&c) + 6.0).abs() < 1.0 && (peak_db(&f) + 6.0).abs() < 1.0, "fs {fs}");
    }
}

#[test]
fn the_same_hits_give_bit_identical_output_whatever_the_block_size() {
    let m = model(48000.0);
    let hits = [(0, hit(0.9, 0.2, 0.3)), (5000, hit(0.4, 0.7, 2.0)), (5001, hit(0.2, 0.5, 1.0)), (20000, hit(1.0, 0.85, 4.0))];
    let reference = render(&m, &hits, 48000, 512);
    for block in [1usize, 64, 100, 128, 1000, 4096] {
        let r = render(&m, &hits, 48000, block);
        assert!(r.0 == reference.0 && r.1 == reference.1, "block {block}");
    }
    // and twice, from a fresh model: the same bits
    let again = render(&model(48000.0), &hits, 48000, 512);
    assert!(again == reference, "deterministic");
    assert!(reference.0.iter().any(|x| *x != 0.0));
}

#[test]
fn a_hit_lands_on_its_sample() {
    let m = model(48000.0);
    let (c, f) = render(&m, &[(777, hit(0.8, 0.3, 0.0))], 4000, 4000);
    assert!(c[..777].iter().chain(&f[..777]).all(|x| *x == 0.0), "silent before the hit");
    assert!(c[777..900].iter().any(|x| *x != 0.0) && f[777..2000].iter().any(|x| *x != 0.0));
}

#[test]
fn the_voice_sleeps_when_it_has_rung_out_and_wakes_clean() {
    let m = model(48000.0);
    let mut v = DrumVoice::new(&m);
    assert!(v.is_sleeping(), "silent before the first hit");
    let (mut c, mut f) = (vec![0f32; 512], vec![0f32; 512]);
    let h = hit(0.7, 0.4, 1.0);
    v.trigger(&m, h);
    let mut first = Vec::new();
    let mut slept_at = None;
    for b in 0..(15.0 * 48000.0 / 512.0) as usize {
        v.process(&m, &mut c, &mut f);
        if b < 40 {
            first.extend_from_slice(&c);
        }
        if slept_at.is_none() && v.is_sleeping() {
            slept_at = Some(b as f64 * 512.0 / 48000.0);
        }
        if let Some(t) = slept_at {
            if b as f64 * 512.0 / 48000.0 > t + 0.05 {
                assert!(c.iter().chain(&f).all(|x| *x == 0.0), "asleep means silent");
            }
        }
    }
    println!("asleep after {:.1} s", slept_at.expect("it falls asleep within 15 s"));
    // a hit wakes it up: and what it plays is what the very first hit played
    v.trigger(&m, h);
    let mut second = Vec::new();
    for _ in 0..40 {
        v.process(&m, &mut c, &mut f);
        second.extend_from_slice(&c);
    }
    assert!(!v.is_sleeping());
    assert!(first == second, "waking clears the state");
}

#[test]
fn stress_through_process_stays_finite() {
    let m = model(44100.0);
    let mut rng = 0x1234ABCD9876u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut v = DrumVoice::new(&m);
    let (mut c, mut f) = (vec![0f32; 256], vec![0f32; 256]);
    let mut peak = 0f32;
    for _ in 0..1500 {
        v.trigger(&m, Hit { velocity: next() as f32, radius: next() * 0.95, angle: next() * 6.283, sample_offset: (next() * 255.0) as u32 });
        for _ in 0..(next() * 6.0) as usize + 1 {
            v.process(&m, &mut c, &mut f);
            peak = c.iter().chain(&f).fold(peak, |a, x| a.max(x.abs()));
        }
    }
    assert_eq!(v.error_count(), 0);
    assert!(peak.is_finite() && peak < 20.0, "{peak}");
}
