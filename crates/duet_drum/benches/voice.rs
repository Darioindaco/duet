use criterion::{criterion_group, criterion_main, Criterion};
use duet_drum::{DrumModel, DrumParams, DrumVoice, Hit};

/// 1 s of audio at 48 kHz, the defaults, a hit every 250 ms, in blocks of 512.
fn one_second(c: &mut Criterion) {
    let fs = 48000.0;
    let m = DrumModel::prepare(&DrumParams::default(), fs).unwrap();
    let block = 512;
    let (mut close, mut far) = (vec![0f32; block], vec![0f32; block]);
    c.bench_function("1 s at 48 kHz, a hit every 250 ms", |b| {
        b.iter(|| {
            let mut v = DrumVoice::new(&m);
            let (mut s, mut k) = (0usize, 0u32);
            while s < 48000 {
                if s % 12000 < block {
                    v.trigger(&m, Hit { velocity: 0.5 + 0.1 * (k % 5) as f32, radius: 0.3 + 0.1 * (k % 4) as f64, angle: k as f64, sample_offset: (s % 12000) as u32 });
                    k += 1;
                }
                v.process(&m, &mut close, &mut far);
                s += block;
            }
            std::hint::black_box((&close, &far));
        })
    });
}

criterion_group! { name = benches; config = Criterion::default().sample_size(10); targets = one_second }
criterion_main!(benches);
