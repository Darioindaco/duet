//! Stage 8 report: mode counts and the speed of `process()` (1 s of audio at 48 kHz, defaults, a hit
//! every 250 ms, one voice, one thread). `cargo run --release --example stage8_report`

use duet_drum::{DrumModel, DrumParams, DrumVoice, Head, Hit, Quality};
use std::time::Instant;

fn speed(m: &DrumModel) -> f64 {
    let fs = m.fs;
    let block = 512;
    let (mut c, mut f) = (vec![0f32; block], vec![0f32; block]);
    let mut best = f64::MAX;
    for _round in 0..5 {
        let mut v = DrumVoice::new(m);
        let t = Instant::now();
        let (mut s, mut k) = (0, 0u32);
        while s < fs as usize {
            if s % 12000 < block {
                v.trigger(m, Hit { velocity: 0.5 + 0.1 * (k % 5) as f32, radius: 0.3 + 0.1 * (k % 4) as f64, angle: k as f64, sample_offset: (s % 12000) as u32 });
                k += 1;
            }
            v.process(m, &mut c, &mut f);
            s += block;
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    best
}

fn main() {
    let fs = 48000.0;
    println!("1 s of audio at 48 kHz, a hit every 250 ms, one voice, one thread (best of 5):\n");
    println!("{:>8} {:>7} {:>8} {:>9} {:>10} {:>10} {:>10}", "quality", "modes", "batter", "resonant", "prepare", "ns/sample", "real time");
    for (name, q) in [("draft", Quality::Draft), ("normal", Quality::Normal), ("high", Quality::High)] {
        let t = Instant::now();
        let m = DrumModel::prepare(&DrumParams::with_quality(q), fs).unwrap();
        let prep = t.elapsed();
        let count = |h: Head| m.modes.iter().filter(|x| x.head == h).count();
        let best = speed(&m);
        println!("{name:>8} {:>7} {:>8} {:>9} {:>8.0?} {:>10.0} {:>9.1}x", m.len(), count(Head::Batter), count(Head::Resonant), prep, best / fs * 1e9, 1.0 / best);
    }
    println!("\n(the spec's target: >= 20x with the spec's cut-offs; `high` is that)");
}
