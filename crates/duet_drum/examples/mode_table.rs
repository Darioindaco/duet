//! The mode table.
//!
//! * `cargo run --release --example mode_table [-- --head resonant] [--params p.json] [--fmax 4000]`:
//!   one head's in-vacuo modes (stage 1).
//! * `cargo run --release --example mode_table -- --json [--params p.json] [--output far|close] [--strike r,θ] [--fs 48000]`:
//!   the prepared drum's modes as JSON, in the format of `tools/analyze.py` (`{source, partials: [{freq, t60, amp, ...}]}`),
//!   to compare with a recording (`tools/compare.py`). `amp` is the mode's peak in that output for an impulsive
//!   strike at (r, θ) (linear: |weight·e_i|·Ω_i, the largest = 1).

use duet_drum::{Block, DrumModel, DrumParams, Head, MembraneBasis, MembraneParams};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let params = match get("--params") {
        Some(path) => DrumParams::from_json(&std::fs::read_to_string(&path).expect("read params")).expect("params"),
        None => DrumParams::default(),
    };
    if args.iter().any(|a| a == "--json") {
        json(&params, &get);
        return;
    }
    let head = if get("--head").as_deref() == Some("resonant") { Head::Resonant } else { Head::Batter };
    // 1.6·f_max: the headroom that air loading then lowers into the band (§4.2)
    let f_cut = get("--fmax").map(|s| s.parse().expect("--fmax")).unwrap_or(1.6 * params.head(head).f_max);
    let mp = MembraneParams::from_drum(&params, head);
    let basis = MembraneBasis::build(&mp, f_cut);
    println!(
        "{head:?} head, in vacuo: R = {} m, σ = {:.4} kg/m², T = {} N/m, D = {:.3e} N·m, wave speed sqrt(T/σ) = {:.1} m/s",
        mp.radius, mp.sigma, mp.tension, mp.bending, (mp.tension / mp.sigma).sqrt()
    );
    println!("modes below {f_cut:.0} Hz: {} (m, n) = {} degrees of freedom\n", basis.modes.len(), basis.dof());
    println!("{:>4} {:>3} {:>3} {:>11} {:>9} {:>10} {:>11} {:>12}", "#", "m", "n", "j_mn", "κ (1/m)", "f_vac (Hz)", "N_mn", "a = ∫Φ dA");
    for (i, m) in basis.modes.iter().enumerate() {
        let a = if m.m == 0 { format!("{:+.5}", m.vol_weight) } else { "0 (pair)".to_string() };
        println!("{:>4} {:>3} {:>3} {:>11.6} {:>9.3} {:>10.2} {:>11.4} {:>12}", i + 1, m.m, m.n, m.j, m.kappa, m.f_vac(), m.norm, a);
    }
}

fn json(params: &DrumParams, get: &dyn Fn(&str) -> Option<String>) {
    let fs: f64 = get("--fs").map(|s| s.parse().expect("--fs")).unwrap_or(48000.0);
    let close = get("--output").as_deref() == Some("close");
    let (r, th) = match get("--strike") {
        Some(s) => {
            let v: Vec<f64> = s.split(',').map(|x| x.trim().parse().expect("--strike r,θ")).collect();
            (v[0], v.get(1).copied().unwrap_or(0.0))
        }
        None => (0.5, 0.0),
    };
    let m = DrumModel::prepare(params, fs).expect("prepare");
    let w = m.output_weights(close);
    let e = m.projection(r, th);
    let raw: Vec<f64> = (0..m.len()).map(|i| (w[i] * e[i]).abs() * m.modes[i].omega).collect();
    let peak = raw.iter().cloned().fold(0.0, f64::max).max(1e-300);
    let partials: Vec<serde_json::Value> = m
        .by_frequency()
        .into_iter()
        .map(|i| {
            let x = &m.modes[i];
            serde_json::json!({
                "freq": x.freq(),
                "t60": x.t60(),
                "amp": raw[i] / peak,
                "label": format!("{}({},{}){}", if x.head == Head::Batter { "B" } else { "R" }, x.m, x.n,
                    if x.block == Block::Pair { if x.sin { "s" } else { "c" } } else { "" }),
                "participation_batter": x.participation[0],
            })
        })
        .collect();
    let out = serde_json::json!({
        "source": "model",
        "output": if close { "close" } else { "far" },
        "strike": [r, th],
        "fs": fs,
        "partials": partials,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
