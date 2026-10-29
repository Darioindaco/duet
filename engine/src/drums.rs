//! The drum kit: synthesized as it plays, the way a TR-808 or 909 does it,
//! so every hit is a little different, harder hits are brighter as well as
//! louder, and every pad's tune, decay, tone, level and pan change it at once.
//! Four kits: 808, 909, Punchy (a modern, tight kit), Lo-fi (the 808 through
//! a crusher). A pad can play your own sample instead (from the Browser).
//!
//! Parameters (all 0..1): level, tune, decay, kit, drive, then per pad
//! p<i>_tune, p<i>_decay, p<i>_tone, p<i>_level, p<i>_pan.

use std::f32::consts::PI;

use crate::instr::{Instrument, NoteEv, N_PARAMS, PADS};

pub const KITS: [&str; 4] = ["808", "909", "Punchy", "Lo-fi"];
/// where the kit-wide knobs end and the pads' begin
pub const PAD0: usize = 5;
const MAX_VOICES: usize = 32;

pub fn kit_of(p: f32) -> usize {
    ((p * KITS.len() as f32) as usize).min(KITS.len() - 1)
}

/// The defaults: the kit's knobs, then every pad's (panned like a drummer sees the kit).
pub const fn defaults() -> [f32; N_PARAMS] {
    let mut p = [0f32; N_PARAMS];
    p[0] = 0.8; // level
    p[1] = 0.5; // tune
    p[2] = 0.5; // decay
    p[3] = 0.6; // kit: Punchy
    p[4] = 0.2; // drive
    let mut i = 0;
    while i < 16 {
        let b = PAD0 + i * 5;
        p[b] = 0.5;
        p[b + 1] = 0.5;
        p[b + 2] = 0.5;
        p[b + 3] = 0.8;
        p[b + 4] = match PADS[i].0 {
            42 | 44 | 46 => 0.62, // hats a little right
            49 => 0.64,           // crash right
            51 => 0.36,           // ride left
            41 => 0.36,           // floor tom left ...
            43 => 0.42,
            45 => 0.5,
            47 => 0.56,
            48 => 0.6,
            50 => 0.64, // ... high toms right
            _ => 0.5,
        };
        i += 1;
    }
    p
}

/// Small, fast noise.
#[derive(Clone, Copy)]
pub struct Noise(pub u32);
impl Noise {
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// RBJ biquad.
#[derive(Clone, Copy, Default)]
pub struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}
impl Biquad {
    pub fn new(kind: u8, f: f32, q: f32, sr: f32) -> Self {
        let w = 2.0 * PI * f.clamp(20.0, sr * 0.45) / sr;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let (b, a0, a) = match kind {
            b'h' => ([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], 1.0 + alpha, [-2.0 * c, 1.0 - alpha]),
            b'b' => ([alpha, 0.0, -alpha], 1.0 + alpha, [-2.0 * c, 1.0 - alpha]),
            _ => ([(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0], 1.0 + alpha, [-2.0 * c, 1.0 - alpha]),
        };
        Biquad { b: [b[0] / a0, b[1] / a0, b[2] / a0], a: [a[0] / a0, a[1] / a0], z: [0.0; 2] }
    }
    pub fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The 808's six square waves for hats and cymbals (its metal), as ratios.
const METAL: [f32; 6] = [1.0, 1.4471, 1.6170, 1.9265, 2.5028, 2.6637];

#[derive(Clone, Copy, PartialEq)]
enum Model {
    Kick,
    Snare,
    Clap,
    Hat,
    Tom,
    Rim,
    Crash,
    Ride,
}

/// How a pad sounds in a kit: its model, base pitch, decay (s), and the
/// kit's character (bend: the kick's pitch drop; click; bright: noise colour).
#[derive(Clone, Copy)]
struct Spec {
    model: Model,
    f: f32,
    decay: f32,
    bend: f32,
    click: f32,
    bright: f32,
    gain: f32,
}

fn spec(kit: usize, pad: usize) -> Spec {
    let note = PADS[pad].0;
    let k = if kit == 3 { 0 } else { kit }; // Lo-fi is the 808, crushed
    let s = |model, f, decay, bend, click, bright, gain| Spec { model, f, decay, bend, click, bright, gain };
    match (note, k) {
        (36, 0) => s(Model::Kick, 47.0, 0.42, 2.0, 0.12, 0.3, 1.0),
        (36, 1) => s(Model::Kick, 54.0, 0.2, 3.2, 0.55, 0.6, 1.0),
        (36, _) => s(Model::Kick, 50.0, 0.15, 4.2, 0.75, 0.7, 1.05),
        (37, _) => s(Model::Rim, 1.0, 0.012, 0.0, 0.5, 0.5, 0.7),
        (38, 0) => s(Model::Snare, 182.0, 0.17, 0.0, 0.2, 0.45, 0.85),
        (38, 1) => s(Model::Snare, 198.0, 0.2, 0.25, 0.35, 0.75, 0.9),
        (38, _) => s(Model::Snare, 210.0, 0.19, 0.35, 0.5, 0.65, 0.95),
        (40, 0) => s(Model::Snare, 238.0, 0.11, 0.0, 0.2, 0.55, 0.75),
        (40, _) => s(Model::Snare, 255.0, 0.12, 0.3, 0.4, 0.8, 0.8),
        (39, _) => s(Model::Clap, 1100.0, 0.16, 0.0, 0.0, if k == 1 { 0.7 } else { 0.5 }, 0.9),
        (42, _) => s(Model::Hat, if k == 1 { 360.0 } else { 320.0 }, 0.04, 0.0, 0.0, if k == 0 { 0.4 } else { 0.7 }, 0.55),
        (44, _) => s(Model::Hat, if k == 1 { 360.0 } else { 320.0 }, 0.075, 0.0, 0.0, 0.5, 0.45),
        (46, _) => s(Model::Hat, if k == 1 { 360.0 } else { 320.0 }, 0.42, 0.0, 0.0, if k == 0 { 0.4 } else { 0.7 }, 0.5),
        (49, _) => s(Model::Crash, 410.0, 1.2, 0.0, 0.0, 0.6, 0.5),
        (51, _) => s(Model::Ride, 520.0, 1.5, 0.0, 0.0, 0.5, 0.45),
        (41, _) => s(Model::Tom, 82.0, 0.42, 0.0, 0.1, 0.4, 0.95),
        (43, _) => s(Model::Tom, 100.0, 0.38, 0.0, 0.1, 0.4, 0.95),
        (45, _) => s(Model::Tom, 124.0, 0.34, 0.0, 0.1, 0.4, 0.9),
        (47, _) => s(Model::Tom, 147.0, 0.3, 0.0, 0.1, 0.4, 0.9),
        (48, _) => s(Model::Tom, 175.0, 0.27, 0.0, 0.1, 0.4, 0.85),
        (_, _) => s(Model::Tom, 208.0, 0.24, 0.0, 0.1, 0.4, 0.85),
    }
}

#[derive(Clone, Copy)]
struct Voice {
    pad: usize,
    spec: Spec,
    /// samples since the hit
    t: u32,
    /// how long it rings at most (samples)
    len: u32,
    vel: f32,
    tune: f32,
    decay: f32,
    tone: f32,
    gl: f32,
    gr: f32,
    choke: f32,
    ph: [f32; 6],
    f1: Biquad,
    f2: Biquad,
    f3: Biquad,
    noise: Noise,
    /// own sample on the pad: where it is in it (fractional)
    pos: f32,
}

pub struct DrumKit {
    /// your own sample per pad (mono, at the device rate), else synthesized
    samples: Vec<Option<Vec<f32>>>,
    voices: Vec<Voice>,
    sr: f32,
    seed: u32,
    /// the Lo-fi kit's sample-and-hold
    held: [f32; 2],
    hold_n: u32,
    /// hits since the last block: they need the knobs, which come with it
    pending_hits: Vec<(u8, u8)>,
}

impl DrumKit {
    pub fn new(sr: f32, own: &[Option<String>]) -> (DrumKit, Option<String>) {
        let mut err = None;
        let samples = (0..PADS.len())
            .map(|i| match own.get(i).cloned().flatten() {
                Some(path) => match crate::decode::read_audio(std::path::Path::new(&path)) {
                    Ok(a) => Some(crate::daw::resample(&a.to_mono(), a.sr, sr as u32)),
                    Err(e) => {
                        err = Some(format!("{}: {e}", PADS[i].1));
                        None
                    }
                },
                None => None,
            })
            .collect();
        (DrumKit { samples, voices: Vec::with_capacity(MAX_VOICES), sr, seed: 0x9E37_79B9, held: [0.0; 2], hold_n: 0,
                  pending_hits: Vec::with_capacity(MAX_VOICES) }, err)
    }

    pub fn pad_for(key: u8) -> usize {
        // GM notes 36..51 are the pads; anything else wraps onto them
        (key as i32 - 36).rem_euclid(PADS.len() as i32) as usize
    }

    fn hit(&mut self, pad: usize, vel: f32, p: &[f32; N_PARAMS]) {
        let sr = self.sr;
        let b = PAD0 + pad * 5;
        let kit = kit_of(p[3]);
        let sp = spec(kit, pad);
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let mut noise = Noise(self.seed | 1);
        // no two hits quite alike: a hair of pitch and decay either way
        let (hp, hd) = (1.0 + 0.004 * noise.next(), 1.0 + 0.03 * noise.next());
        let tune = 2f32.powf((p[b] - 0.5) * 24.0 / 12.0 + (p[1] - 0.5)) * hp; // the pad ±12, the kit ±6 semitones
        let decay = 2f32.powf((p[b + 1] - 0.5) * 3.0) * 2f32.powf((p[2] - 0.5) * 2.0) * hd;
        // harder hits are brighter, not just louder
        let tone = (p[b + 2] + (vel - 0.75) * 0.3).clamp(0.0, 1.0);
        let level = p[b + 3] * p[b + 3] * 1.56 * sp.gain;
        let pan = p[b + 4].clamp(0.0, 1.0);
        let (gl, gr) = ((pan * PI / 2.0).cos() * std::f32::consts::SQRT_2, (pan * PI / 2.0).sin() * std::f32::consts::SQRT_2);
        let g = vel.powf(1.6) * level;
        let d = sp.decay * decay;
        let bright = (sp.bright + (tone - 0.5) * 0.8).clamp(0.0, 1.0);
        let (f1, f2, f3) = match sp.model {
            Model::Kick => (Biquad::new(b'h', 1200.0, 0.7, sr), Biquad::default(), Biquad::default()),
            Model::Snare => (Biquad::new(b'h', 1200.0 + bright * 1800.0, 0.7, sr), Biquad::new(b'l', 6000.0 + bright * 8000.0, 0.7, sr), Biquad::default()),
            Model::Clap => (Biquad::new(b'b', sp.f * (0.7 + bright * 0.9), 1.6, sr), Biquad::new(b'h', 500.0, 0.7, sr), Biquad::default()),
            Model::Hat => (Biquad::new(b'b', 9000.0 + bright * 2500.0, 1.0, sr), Biquad::new(b'h', 5500.0 + bright * 3500.0, 0.7, sr), Biquad::new(b'h', 7000.0, 0.7, sr)),
            Model::Crash => (Biquad::new(b'h', 3500.0 + bright * 3000.0, 0.7, sr), Biquad::default(), Biquad::default()),
            Model::Ride => (Biquad::new(b'b', 4500.0 + bright * 2500.0, 0.9, sr), Biquad::default(), Biquad::default()),
            Model::Rim => (Biquad::new(b'b', 1700.0 * tune, 8.0, sr), Biquad::new(b'b', 520.0 * tune, 6.0, sr), Biquad::default()),
            Model::Tom => (Biquad::new(b'l', 3000.0, 0.7, sr), Biquad::default(), Biquad::default()),
        };
        let len = match &self.samples[pad] {
            Some(s) => (s.len() as f32 / tune) as u32,
            None => ((d * 7.0).min(10.0) * sr) as u32 + (0.1 * sr) as u32, // to -60 dB (at most 10 s), and a fade
        };
        // a closed or pedal hat cuts an open one
        if matches!(PADS[pad].0, 42 | 44) {
            for v in self.voices.iter_mut().filter(|v| PADS[v.pad].0 == 46) {
                v.choke = v.choke.min(0.999);
            }
        }
        if self.voices.len() >= MAX_VOICES {
            self.voices.remove(0); // the oldest makes room (capacity is reserved: no allocation)
        }
        let spec = Spec { decay: d, bright, ..sp };
        self.voices.push(Voice { pad, spec, t: 0, len, vel: g, tune, decay, tone, gl, gr, choke: 1.0, ph: [0.0; 6], f1, f2, f3, noise, pos: 0.0 });
    }
}

impl Voice {
    /// The next sample of a synthesized voice.
    fn tick(&mut self, sr: f32) -> f32 {
        let t = self.t as f32 / sr;
        let sp = self.spec;
        let d = sp.decay;
        let n = self.noise.next();
        let x = match sp.model {
            Model::Kick => {
                // a sine dropping into its note, a click on top, pushed into saturation
                let f = sp.f * self.tune * (1.0 + (sp.bend + 0.4 * self.vel) * (-t / 0.018).exp());
                self.ph[0] = (self.ph[0] + f / sr).fract();
                let body = (2.0 * PI * self.ph[0]).sin() * (-t / d).exp();
                let click = self.f1.run(n) * (-t / 0.0025).exp() * sp.click * (0.4 + self.tone);
                ((body + click) * (1.0 + self.tone * 1.2)).tanh()
            }
            Model::Snare => {
                // two tuned heads, and the wires: filtered noise
                let drop = 1.0 + sp.bend * (-t / 0.01).exp();
                self.ph[0] = (self.ph[0] + sp.f * self.tune * drop / sr).fract();
                self.ph[1] = (self.ph[1] + sp.f * 1.82 * self.tune * drop / sr).fract();
                let heads = ((2.0 * PI * self.ph[0]).sin() * (-t / (0.6 * d)).exp() + 0.55 * (2.0 * PI * self.ph[1]).sin() * (-t / (0.4 * d)).exp()) * 0.6;
                let wires = self.f2.run(self.f1.run(n)) * (-t / d).exp() * (0.55 + self.tone * 0.9);
                let crack = n * (-t / 0.002).exp() * sp.click;
                heads * (1.2 - self.tone * 0.5) + wires + crack
            }
            Model::Clap => {
                // a few hands not quite together, then the room
                let mut env = 0.0f32;
                for s in [0.0f32, 0.0095, 0.019, 0.0275] {
                    if t >= s {
                        env = env.max((-(t - s) / 0.0045).exp());
                    }
                }
                if t > 0.03 {
                    env = env.max(0.75 * (-(t - 0.03) / d).exp());
                }
                self.f2.run(self.f1.run(n)) * env * 2.6
            }
            Model::Hat => {
                let mut m = 0.0;
                for (i, r) in METAL.iter().enumerate() {
                    self.ph[i] = (self.ph[i] + sp.f * r * self.tune / sr).fract();
                    m += if self.ph[i] < 0.5 { 1.0 } else { -1.0 };
                }
                let src = m / 6.0 * (1.0 - sp.bright * 0.4) + n * sp.bright * 0.5;
                self.f3.run(self.f2.run(self.f1.run(src))) * (-t / d).exp() * 2.4
            }
            Model::Crash => {
                let mut m = 0.0;
                for (i, r) in METAL.iter().enumerate() {
                    self.ph[i] = (self.ph[i] + sp.f * r * self.tune / sr).fract();
                    m += if self.ph[i] < 0.5 { 1.0 } else { -1.0 };
                }
                let env = (-t / d).exp() * 0.8 + (-t / 0.02).exp() * 0.4;
                self.f1.run(m / 6.0 * 0.5 + n * 0.6) * env * 1.6
            }
            Model::Ride => {
                let mut m = 0.0;
                for (i, r) in METAL.iter().enumerate() {
                    self.ph[i] = (self.ph[i] + sp.f * r * self.tune / sr).fract();
                    m += if self.ph[i] < 0.5 { 1.0 } else { -1.0 };
                }
                let bell = (2.0 * PI * 3300.0 * self.tune * t).sin() * (-t / 0.45).exp() * (0.1 + self.tone * 0.15);
                self.f1.run(m / 6.0 * 0.6 + n * 0.3) * (-t / d).exp() * 1.7 + bell
            }
            Model::Rim => {
                let ping = self.f1.run(n) * 4.0 + self.f2.run(n) * 3.0;
                ping * (-t / (d * 1.4)).exp() + n * (-t / 0.0015).exp() * 0.5
            }
            Model::Tom => {
                let f = sp.f * self.tune * (1.0 + 0.55 * (-t / 0.045).exp());
                self.ph[0] = (self.ph[0] + f / sr).fract();
                let body = (2.0 * PI * self.ph[0]).sin() + 0.12 * (4.0 * PI * self.ph[0]).sin() * self.tone;
                (body * (-t / d).exp() + self.f1.run(n) * (-t / 0.012).exp() * (0.1 + 0.2 * self.tone)) * 1.1
            }
        };
        self.t += 1;
        x
    }
}

impl Instrument for DrumKit {
    fn note(&mut self, ev: NoteEv) {
        // (hits come through `play`, which knows the knobs; see process)
        match ev {
            NoteEv::On(key, vel) => self.pending(key, vel),
            NoteEv::AllOff => self.voices.clear(),
            _ => {}
        }
    }

    fn process(&mut self, p: &[f32; N_PARAMS], out: &mut [f32], out_r: &mut [f32]) {
        out.fill(0.0);
        out_r.fill(0.0);
        // hits that came in since the last block (the knobs are known here)
        let mut pend = std::mem::take(&mut self.pending_hits); // (no allocation: it's put back)
        for &(key, vel) in pend.iter() {
            self.hit(Self::pad_for(key), vel as f32 / 127.0, p);
        }
        pend.clear();
        self.pending_hits = pend;
        let sr = self.sr;
        let level = p[0] * p[0] * 1.4;
        for v in self.voices.iter_mut() {
            for (o, o_r) in out.iter_mut().zip(out_r.iter_mut()) {
                let x = match &self.samples[v.pad] {
                    Some(s) => {
                        // your own sample: played at the pad's tune, shortened by its decay
                        let i = v.pos as usize;
                        if i + 1 >= s.len() {
                            v.choke = 0.0;
                            break;
                        }
                        let fr = v.pos - i as f32;
                        let y = s[i] + (s[i + 1] - s[i]) * fr;
                        v.pos += v.tune;
                        let t = v.t as f32 / sr;
                        v.t += 1;
                        // (decay at 1 leaves it as it is; below, it's faded sooner)
                        if v.decay >= 0.999 { y } else { y * (-t / (1.2 * v.decay)).exp() }
                    }
                    None => {
                        // the last 100 ms fade out (a long ride or crash doesn't just stop)
                        let left = v.len.saturating_sub(v.t) as f32;
                        v.tick(sr) * (left / (0.1 * sr)).min(1.0)
                    }
                };
                let g = v.vel * v.choke * level;
                *o += x * g * v.gl;
                *o_r += x * g * v.gr;
                if v.choke < 1.0 {
                    v.choke *= 0.995; // ~5 ms fade when choked
                }
                if v.t >= v.len {
                    v.choke = 0.0;
                    break;
                }
            }
        }
        self.voices.retain(|v| v.choke > 1e-3 && v.t < v.len);
        // the kit: a little drive, and Lo-fi's crusher
        let drive = p[4];
        let lofi = kit_of(p[3]) == 3;
        for (l, r) in out.iter_mut().zip(out_r.iter_mut()) {
            if lofi {
                if self.hold_n == 0 {
                    let q = |x: f32| (x * 48.0).round() / 48.0; // ~6 bits
                    self.held = [q(*l), q(*r)];
                }
                self.hold_n = (self.hold_n + 1) % 4; // a quarter of the rate
                *l = self.held[0];
                *r = self.held[1];
            }
            if drive > 0.001 {
                let k = 1.0 + drive * 4.0;
                *l = (*l * k).tanh() / k.tanh().max(0.3) * 0.9;
                *r = (*r * k).tanh() / k.tanh().max(0.3) * 0.9;
            }
        }
    }
}

impl DrumKit {
    fn pending(&mut self, key: u8, vel: u8) {
        if self.pending_hits.len() < MAX_VOICES {
            self.pending_hits.push((key, vel));
        }
    }
}

/// A pad's sound in the default kit, rendered (the Browser's built-in sounds).
pub fn render_pad(pad: usize, sr: f32) -> Vec<f32> {
    let (mut kit, _) = DrumKit::new(sr, &[]);
    let p = defaults();
    kit.note(NoteEv::On(PADS[pad].0, 110));
    let mut out = Vec::new();
    let (mut l, mut r) = ([0f32; 256], [0f32; 256]);
    loop {
        kit.process(&p, &mut l, &mut r);
        out.extend(l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)));
        if kit.voices.is_empty() || out.len() > (sr * 4.0) as usize {
            break;
        }
    }
    let peak = out.iter().fold(0f32, |a, v| a.max(v.abs())).max(1e-6);
    out.iter().map(|v| v * 0.9 / peak).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kit: &mut DrumKit, p: &[f32; N_PARAMS], blocks: usize) -> Vec<f32> {
        let (mut l, mut r) = (Vec::new(), [0f32; 128]);
        let mut b = [0f32; 128];
        for _ in 0..blocks {
            kit.process(p, &mut b, &mut r);
            l.extend_from_slice(&b);
        }
        l
    }
    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |a, v| a.max(v.abs()))
    }

    #[test]
    fn every_kit_sounds_every_pad_and_every_knob_stays_sane() {
        for k in 0..KITS.len() {
            for pad in 0..PADS.len() {
                for (knob, x) in [(None, 0.0), (Some(0), 0.0), (Some(0), 1.0), (Some(1), 0.0), (Some(1), 1.0), (Some(2), 0.0), (Some(2), 1.0)] {
                    let mut p = defaults();
                    p[3] = (k as f32 + 0.5) / KITS.len() as f32;
                    p[4] = 1.0; // the most drive
                    if let Some(j) = knob {
                        p[PAD0 + pad * 5 + j] = x;
                    }
                    let (mut kit, _) = DrumKit::new(48000.0, &[]);
                    kit.note(NoteEv::On(PADS[pad].0, 127));
                    let x = run(&mut kit, &p, 200);
                    let what = format!("{} {} knob {knob:?}", KITS[k], PADS[pad].1);
                    assert!(x.iter().all(|v| v.is_finite() && v.abs() < 2.0), "{what}");
                    assert!(peak(&x) > 0.05, "{what}: silent");
                    // it ends (10 s at most), and quietly
                    let mut tail = Vec::new();
                    for _ in 0..40 {
                        if kit.voices.is_empty() {
                            break;
                        }
                        tail = run(&mut kit, &p, 100);
                    }
                    assert!(kit.voices.is_empty(), "{what}: rings on");
                    assert!(tail.is_empty() || peak(&tail[tail.len() - 256..]) < 2e-3, "{what}: stops loud");
                }
            }
        }
    }

    #[test]
    fn harder_hits_are_louder_and_the_pad_level_and_pan_work() {
        let p = defaults();
        let hit = |p: &[f32; N_PARAMS], vel| {
            let (mut kit, _) = DrumKit::new(48000.0, &[]);
            kit.note(NoteEv::On(38, vel));
            let (mut l, mut r) = (vec![0f32; 4800], vec![0f32; 4800]);
            kit.process(p, &mut l, &mut r);
            (peak(&l), peak(&r))
        };
        assert!(hit(&p, 127).0 > hit(&p, 40).0 * 1.5);
        let mut q = p;
        q[PAD0 + 2 * 5 + 3] = 0.0; // the snare's level
        assert!(hit(&q, 127).0 < 1e-4);
        q = p;
        q[PAD0 + 2 * 5 + 4] = 0.0; // the snare all the way left
        let (l, r) = hit(&q, 127);
        assert!(l > 0.05 && r < 1e-4, "{l} {r}");
    }
}
