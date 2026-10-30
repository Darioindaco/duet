//! Renders hits to WAV: `cargo run --release --example render -- [--params p.json] [--hits hits.json |
//! --hit vel,r,θ[,time] ...] [--seconds 3] [--fs 48000] [--quality draft|normal|high] [--out renders/out]`
//!
//! Writes `<out>_close.wav` and `<out>_far.wav` (mono, 32-bit float, the output gains of the parameters).
//! A hits file is `[{"time": 0.0, "velocity": 0.7, "radius": 0.5, "angle": 0.0}, ...]` (time in seconds).

use duet_drum::{DrumModel, DrumParams, DrumVoice, Hit, Quality};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let mut params = match get("--quality").as_deref() {
        Some("draft") => DrumParams::with_quality(Quality::Draft),
        Some("normal") => DrumParams::with_quality(Quality::Normal),
        _ => DrumParams::default(),
    };
    if let Some(path) = get("--params") {
        // (a params file sets everything it names; the quality's cut-offs stay unless it names f_max)
        let patch: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read params")).expect("params JSON");
        let mut base = serde_json::to_value(&params).unwrap();
        merge(&mut base, patch);
        params = DrumParams::from_json(&base.to_string()).expect("params");
    }
    let fs: f64 = get("--fs").map(|s| s.parse().expect("--fs")).unwrap_or(48000.0);
    let seconds: f64 = get("--seconds").map(|s| s.parse().expect("--seconds")).unwrap_or(3.0);
    let out = get("--out").unwrap_or_else(|| "renders/out".into());
    // the hits: a file, or --hit vel,r,θ[,time] (repeatable)
    let mut hits: Vec<(f64, Hit)> = Vec::new();
    if let Some(path) = get("--hits") {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read hits")).expect("hits JSON");
        for h in v.as_array().expect("a list of hits") {
            let f = |k: &str, d: f64| h.get(k).and_then(|x| x.as_f64()).unwrap_or(d);
            hits.push((f("time", 0.0), Hit { velocity: f("velocity", 0.7) as f32, radius: f("radius", 0.5), angle: f("angle", 0.0), sample_offset: 0 }));
        }
    }
    for (i, a) in args.iter().enumerate() {
        if a == "--hit" {
            let v: Vec<f64> = args[i + 1].split(',').map(|x| x.trim().parse().expect("--hit vel,r,θ[,time]")).collect();
            hits.push((v.get(3).copied().unwrap_or(0.0), Hit { velocity: v[0] as f32, radius: v[1], angle: v.get(2).copied().unwrap_or(0.0), sample_offset: 0 }));
        }
    }
    if hits.is_empty() {
        hits.push((0.0, Hit { velocity: 0.7, radius: 0.5, angle: 0.0, sample_offset: 0 }));
    }
    let m = DrumModel::prepare(&params, fs).expect("prepare");
    for w in &m.warnings {
        eprintln!("warning: {w}");
    }
    let total = (seconds * fs) as usize;
    let block = 512;
    let mut v = DrumVoice::new(&m);
    let (mut close, mut far) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut c, mut f) = (vec![0f32; block], vec![0f32; block]);
    let mut t = 0;
    while t < total {
        let len = block.min(total - t);
        for (at, h) in &hits {
            let s = (at * fs).round() as usize;
            if s >= t && s < t + len {
                v.trigger(&m, Hit { sample_offset: (s - t) as u32, ..*h });
            }
        }
        v.process(&m, &mut c[..len], &mut f[..len]);
        close.extend_from_slice(&c[..len]);
        far.extend_from_slice(&f[..len]);
        t += len;
    }
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).ok();
    }
    for (name, x) in [("close", &close), ("far", &far)] {
        let spec = hound::WavSpec { channels: 1, sample_rate: fs as u32, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let path = format!("{out}_{name}.wav");
        let mut w = hound::WavWriter::create(&path, spec).expect("create WAV");
        for s in x.iter() {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();
        let peak = x.iter().fold(1e-9f32, |a, s| a.max(s.abs()));
        eprintln!("{path}: {} hits, {} modes, peak {:.1} dBFS", hits.len(), m.len(), 20.0 * (peak as f64).log10());
    }
}

fn merge(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(b), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (slot, v) => *slot = v,
    }
}
