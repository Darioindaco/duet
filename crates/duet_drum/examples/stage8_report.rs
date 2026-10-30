//! Stage 8 report: mode counts and the speed of `process()` (1 s of audio at 48 kHz, defaults, a hit
//! every 250 ms, one voice, one thread). `cargo run --release --example stage8_report`

use duet_drum::{DrumModel, DrumParams, DrumVoice, Head, Hit};
use std::time::Instant;

fn main() {
    let fs = 48000.0;
    let t = Instant::now();
    let m = DrumModel::prepare(&DrumParams::default(), fs).unwrap();
    let prep = t.elapsed();
    let count = |h: Head| m.modes.iter().filter(|x| x.head == h).count();
    println!("modes: {} in all: batter {}, resonant {}; {} axisymmetric (the dense block); prepare {:.0?}", m.len(), count(Head::Batter), count(Head::Resonant), m.n_axisym(), prep);
    let block = 512;
    let (mut c, mut f) = (vec![0f32; block], vec![0f32; block]);
    let mut best = f64::MAX;
    for _round in 0..5 {
        let mut v = DrumVoice::new(&m);
        let t = Instant::now();
        let mut s = 0;
        let mut k = 0u32;
        while s < fs as usize {
            if s % 12000 < block {
                v.trigger(&m, Hit { velocity: 0.5 + 0.1 * (k % 5) as f32, radius: 0.3 + 0.1 * (k % 4) as f64, angle: k as f64, sample_offset: (s % 12000) as u32 });
                k += 1;
            }
            v.process(&m, &mut c, &mut f);
            s += block;
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    println!("1 s of audio: {:.1} ms (best of 5) = {:.0} ns/sample, {:.1}× real time", best * 1e3, best / fs * 1e9, 1.0 / best);
}
