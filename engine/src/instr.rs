//! Instruments: what a strip plays instead of an interface input. Notes come
//! from a MIDI keyboard, the computer keyboard or (later) the song.
//!
//! Same realtime rule as the effects: built (and, for the drum kit, rendered)
//! on the control side, handed to the audio thread in a Box, handed back to be
//! freed. Output is mono, like every strip.

use std::f32::consts::PI;

use crate::fx::VstRef;

pub const DRUMS: usize = 0;
pub const SYNTH: usize = 1;
pub const VSTI: usize = 2;
pub const KINDS: [&str; 3] = ["drums", "synth", "vst"];
pub const N_PARAMS: usize = 6;

/// Knobs per kind (0..1), and where they start.
pub const PARAMS: [&[&str]; 3] = [
    &["level", "tune", "decay"],
    &["wave", "cutoff", "resonance", "attack", "release", "detune"],
    &[],
];
pub const DEFAULTS: [[f32; N_PARAMS]; 3] = [
    [0.8, 0.5, 0.5, 0.0, 0.0, 0.0],
    [0.3, 0.55, 0.25, 0.05, 0.3, 0.2],
    [0.0; N_PARAMS],
];

pub fn kind_index(name: &str) -> Option<usize> {
    KINDS.iter().position(|k| *k == name)
}

/// The drum kit's pads: General MIDI notes 36..51, so drum parts and pad
/// controllers that follow GM land on the right sound.
pub const PADS: [(u8, &str); 16] = [
    (36, "Kick"), (37, "Rim"), (38, "Snare"), (39, "Clap"),
    (40, "Snare 2"), (41, "Floor tom"), (42, "Closed hat"), (43, "Low tom"),
    (44, "Pedal hat"), (45, "Mid tom"), (46, "Open hat"), (47, "Tom"),
    (48, "High tom"), (49, "Crash"), (50, "Top tom"), (51, "Ride"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoteEv {
    On(u8, u8), // key, velocity
    Off(u8),
    Sustain(bool),
    /// let every sounding note go (stop, a loop jumping back): releases ring out
    Release,
    /// silence now (panic)
    AllOff,
}

/// What an instrument slot holds, without its DSP: the control side's mirror.
#[derive(Clone, Debug, PartialEq)]
pub struct InstCfg {
    pub kind: usize,
    pub p: [f32; N_PARAMS],
    pub vst: Option<VstRef>,
    /// drum kit: your own sample per pad (a WAV path), None = the built-in sound
    pub pads: Vec<Option<String>>,
    pub error: Option<String>,
}

impl InstCfg {
    pub fn new(kind: usize) -> Self {
        InstCfg { kind, p: DEFAULTS[kind], vst: None, pads: vec![None; PADS.len()], error: None }
    }
}

pub trait Instrument: Send {
    fn note(&mut self, ev: NoteEv);
    /// Write one block (replaces what's in `out`).
    fn process(&mut self, p: &[f32; N_PARAMS], out: &mut [f32]);
    /// Instruments that can follow a sample-rate change in place say so; the
    /// rest are rebuilt.
    fn set_sample_rate(&mut self, _sr: f32) -> bool {
        false
    }
}

/// An instrument as the audio thread holds it.
pub struct InstSlot {
    pub cfg: InstCfg,
    pub inst: Box<dyn Instrument>,
}

impl InstSlot {
    /// Builds the instrument. Allocates (renders the kit, loads samples or a
    /// plugin), so never on the audio thread. A pad sample that can't be read
    /// falls back to the built-in sound and says so in `cfg.error`.
    pub fn new(mut cfg: InstCfg, sr: f32) -> Result<Box<InstSlot>, String> {
        cfg.error = None;
        let inst: Box<dyn Instrument> = match cfg.kind {
            DRUMS => {
                let (kit, err) = DrumKit::new(sr, &cfg.pads);
                cfg.error = err;
                Box::new(kit)
            }
            SYNTH => Box::new(Synth::new(sr)),
            VSTI => {
                let v = cfg.vst.clone().ok_or("no plugin chosen")?;
                let state = v.state.as_deref().and_then(crate::vst::State::decode);
                let proc = crate::vst::VstProc::create_as(&v.path, &v.cid, state, sr as f64, true)?;
                let inst = proc.instance();
                cfg.vst = Some(VstRef {
                    name: if inst.name.is_empty() { v.name } else { inst.name.clone() },
                    id: proc.id,
                    latency: inst.latency,
                    has_editor: inst.has_editor,
                    ..v
                });
                Box::new(VstInst { proc, pending: [None; 160], n_pending: 0, sustain: false, held: [false; 128], sounding: [false; 128] })
            }
            k => return Err(format!("no instrument kind {k}")),
        };
        Ok(Box::new(InstSlot { cfg, inst }))
    }
}

// --- the drum kit -------------------------------------------------------------------

struct Voice {
    pad: usize,
    pos: usize,
    gain: f32,
    choke: f32, // 1 = playing; falls to 0 once choked
}

pub struct DrumKit {
    samples: Vec<Vec<f32>>,
    voices: Vec<Voice>,
    sr: f32,
}

const MAX_DRUM_VOICES: usize = 32;

impl DrumKit {
    pub fn new(sr: f32, own: &[Option<String>]) -> (DrumKit, Option<String>) {
        let mut err = None;
        let samples = (0..PADS.len())
            .map(|i| match own.get(i).cloned().flatten() {
                Some(path) => match crate::daw::read_wav(std::path::Path::new(&path)) {
                    Ok(a) => crate::daw::resample(&a.samples, a.sr, sr as u32),
                    Err(e) => {
                        err = Some(format!("{}: {e}", PADS[i].1));
                        drum_sound(i, sr)
                    }
                },
                None => drum_sound(i, sr),
            })
            .collect();
        (DrumKit { samples, voices: Vec::with_capacity(MAX_DRUM_VOICES), sr }, err)
    }

    pub fn pad_for(key: u8) -> usize {
        // GM notes 36..51 are the pads; anything else wraps onto them
        (key as i32 - 36).rem_euclid(PADS.len() as i32) as usize
    }
}

impl Instrument for DrumKit {
    fn note(&mut self, ev: NoteEv) {
        let NoteEv::On(key, vel) = ev else {
            if ev == NoteEv::AllOff {
                self.voices.clear();
            }
            return;
        };
        let pad = Self::pad_for(key);
        // hi-hats share one "pedal": a closed or pedal hat cuts an open one
        if matches!(PADS[pad].0, 42 | 44) {
            for v in self.voices.iter_mut().filter(|v| PADS[v.pad].0 == 46) {
                v.choke = v.choke.min(0.999);
            }
        }
        if self.voices.len() >= MAX_DRUM_VOICES {
            self.voices.remove(0); // the oldest makes room (capacity is reserved: no allocation)
        }
        let g = vel as f32 / 127.0;
        self.voices.push(Voice { pad, pos: 0, gain: g * g, choke: 1.0 });
    }

    fn process(&mut self, p: &[f32; N_PARAMS], out: &mut [f32]) {
        out.fill(0.0);
        let level = p[0] * p[0] * 1.6;
        // tune: ±6 semitones by stepping through the sample faster or slower
        let rate = 2f32.powf((p[1] - 0.5) * 12.0 / 12.0);
        // decay: shortens (or keeps) every sound
        let decay_s = 0.05 + p[2] * 4.0;
        let fade_per = 1.0 / (decay_s * self.sr);
        for v in self.voices.iter_mut() {
            let s = &self.samples[v.pad];
            for o in out.iter_mut() {
                let idx = v.pos as f32 * rate;
                let i = idx as usize;
                if i + 1 >= s.len() {
                    v.gain = 0.0;
                    break;
                }
                let t = idx - i as f32;
                let x = s[i] + (s[i + 1] - s[i]) * t;
                let env = (1.0 - v.pos as f32 * fade_per).max(0.0);
                *o += x * v.gain * level * env * v.choke;
                if v.choke < 1.0 {
                    v.choke *= 0.995; // ~5 ms fade when choked
                }
                v.pos += 1;
            }
            if v.choke < 1e-3 || (1.0 - v.pos as f32 * fade_per) <= 0.0 {
                v.gain = 0.0;
            }
        }
        self.voices.retain(|v| v.gain > 0.0);
    }
}

/// Small deterministic noise source for the synthesized kit.
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// RBJ biquad, for shaping noise.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}
impl Biquad {
    fn new(kind: &str, f: f32, q: f32, sr: f32) -> Self {
        let w = 2.0 * PI * f / sr;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let (b, a0, a) = match kind {
            "hp" => ([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], 1.0 + alpha, [-2.0 * c, 1.0 - alpha]),
            "bp" => ([alpha, 0.0, -alpha], 1.0 + alpha, [-2.0 * c, 1.0 - alpha]),
            _ => ([(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0], 1.0 + alpha, [-2.0 * c, 1.0 - alpha]),
        };
        Biquad { b: [b[0] / a0, b[1] / a0, b[2] / a0], a: [a[0] / a0, a[1] / a0], z: [0.0; 2] }
    }
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// An 808-style "metal" tone: six detuned square waves (hats, cymbals, cowbell).
fn metal(t: f32, base: f32) -> f32 {
    const R: [f32; 6] = [1.0, 1.4471, 1.6170, 1.9265, 2.5028, 2.6637];
    R.iter().map(|r| if (t * base * r).fract() < 0.5 { 1.0 } else { -1.0 }).sum::<f32>() / 6.0
}

/// A tom: a sine that drops in pitch, plus a little noise.
fn tom(sr: f32, f: f32, n: usize, noise: &mut Noise) -> Vec<f32> {
    let mut ph = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            ph += 2.0 * PI * f * (1.0 + 0.6 * (-t / 0.05).exp()) / sr;
            (ph.sin() + 0.06 * noise.next()) * (-t / 0.32).exp()
        })
        .collect()
}

/// The built-in sound of pad `i`, rendered at `sr`.
pub fn drum_sound(i: usize, sr: f32) -> Vec<f32> {
    let len = |s: f32| (s * sr) as usize;
    let mut noise = Noise(0x9E37_79B9 ^ (i as u32 + 1).wrapping_mul(2_654_435_761));
    let mut out: Vec<f32> = match PADS[i].0 {
        36 => {
            // kick: a sine sweeping down, a click on top, softly saturated
            let mut ph = 0.0f32;
            (0..len(0.9))
                .map(|k| {
                    let t = k as f32 / sr;
                    ph += 2.0 * PI * (45.0 + 115.0 * (-t / 0.032).exp()) / sr;
                    let click = if t < 0.004 { noise.next() * 0.35 * (1.0 - t / 0.004) } else { 0.0 };
                    ((ph.sin() * (-t / 0.30).exp() + click) * 1.6).tanh()
                })
                .collect()
        }
        37 => {
            // rim: two short tones and a click
            (0..len(0.12))
                .map(|k| {
                    let t = k as f32 / sr;
                    ((2.0 * PI * 1720.0 * t).sin() * 0.6 + (2.0 * PI * 820.0 * t).sin() * 0.5) * (-t / 0.012).exp()
                        + noise.next() * 0.3 * (-t / 0.003).exp()
                })
                .collect()
        }
        38 | 40 => {
            // snares: a body tone and a burst of bright noise
            let (f, nd, td) = if PADS[i].0 == 38 { (185.0, 0.17, 0.09) } else { (230.0, 0.10, 0.06) };
            let mut hp = Biquad::new("hp", 1400.0, 0.7, sr);
            (0..len(0.45))
                .map(|k| {
                    let t = k as f32 / sr;
                    let tone = ((2.0 * PI * f * t).sin() + 0.5 * (2.0 * PI * f * 1.78 * t).sin()) * (-t / td).exp() * 0.55;
                    tone + hp.run(noise.next()) * (-t / nd).exp() * 0.9
                })
                .collect()
        }
        39 => {
            // clap: three quick bursts of band-passed noise, then a tail
            let mut bp = Biquad::new("bp", 1150.0, 1.4, sr);
            (0..len(0.4))
                .map(|k| {
                    let t = k as f32 / sr;
                    let bursts: f32 = [0.0, 0.011, 0.022].iter().map(|s| if t >= *s { (-(t - s) / 0.007).exp() } else { 0.0 }).sum();
                    let env = bursts.max((-(t - 0.022).max(0.0) / 0.13).exp() * if t > 0.022 { 0.7 } else { 0.0 });
                    bp.run(noise.next()) * env * 2.2
                })
                .collect()
        }
        41 => tom(sr, 82.0, len(0.8), &mut noise),
        43 => tom(sr, 100.0, len(0.75), &mut noise),
        45 => tom(sr, 122.0, len(0.7), &mut noise),
        47 => tom(sr, 145.0, len(0.65), &mut noise),
        48 => tom(sr, 172.0, len(0.6), &mut noise),
        50 => tom(sr, 205.0, len(0.55), &mut noise),
        42 | 44 | 46 => {
            // hats: metal through a high-pass; closed, pedal, open
            let d = match PADS[i].0 { 42 => 0.045, 44 => 0.075, _ => 0.42 };
            let mut hp = Biquad::new("hp", 7000.0, 0.8, sr);
            (0..len(d * 5.0))
                .map(|k| {
                    let t = k as f32 / sr;
                    hp.run(metal(t, 320.0) * 0.7 + noise.next() * 0.4) * (-t / d).exp() * 1.4
                })
                .collect()
        }
        49 => {
            let mut hp = Biquad::new("hp", 4200.0, 0.7, sr);
            (0..len(2.6))
                .map(|k| {
                    let t = k as f32 / sr;
                    hp.run(metal(t, 410.0) * 0.5 + noise.next() * 0.6) * (-t / 0.75).exp() * 1.3
                })
                .collect()
        }
        _ => {
            // ride: a bell ping over a washy cymbal
            let mut bp = Biquad::new("bp", 5200.0, 0.9, sr);
            (0..len(2.2))
                .map(|k| {
                    let t = k as f32 / sr;
                    bp.run(metal(t, 520.0) * 0.6 + noise.next() * 0.3) * (-t / 0.6).exp() * 1.5
                        + (2.0 * PI * 3300.0 * t).sin() * (-t / 0.35).exp() * 0.12
                })
                .collect()
        }
    };
    let peak = out.iter().fold(0f32, |a, v| a.max(v.abs())).max(1e-6);
    let g = 0.9 / peak;
    // and a short fade at the very end, so no sound ever stops with a click
    let tail = len(0.03).min(out.len());
    let n = out.len();
    for (k, v) in out.iter_mut().enumerate() {
        let fade = if k + tail >= n { (n - k) as f32 / tail as f32 } else { 1.0 };
        *v *= g * fade;
    }
    out
}

// --- the synth -----------------------------------------------------------------------

#[derive(Clone, Copy)]
struct SVoice {
    key: u8,
    on: bool,     // key still down (or held by the pedal)
    held: bool,   // released while the pedal was down
    vel: f32,
    ph: [f32; 2], // two oscillators, the second detuned
    env: f32,
    age: u64,
    ic1: f32,     // filter state
    ic2: f32,
}

pub struct Synth {
    voices: [Option<SVoice>; 8],
    sr: f32,
    sustain: bool,
    clock: u64,
}

impl Synth {
    pub fn new(sr: f32) -> Self {
        Synth { voices: [None; 8], sr, sustain: false, clock: 0 }
    }
}

fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

impl Instrument for Synth {
    fn note(&mut self, ev: NoteEv) {
        match ev {
            NoteEv::On(key, vel) => {
                self.clock += 1;
                let v = SVoice { key, on: true, held: false, vel: vel as f32 / 127.0, ph: [0.0, 0.0], env: 0.0, age: self.clock, ic1: 0.0, ic2: 0.0 };
                // same key again, a free voice, or steal the oldest
                let slot = self.voices.iter().position(|x| x.is_some_and(|x| x.key == key))
                    .or_else(|| self.voices.iter().position(|x| x.is_none()))
                    .unwrap_or_else(|| (0..8).min_by_key(|&i| self.voices[i].map_or(0, |x| x.age)).unwrap());
                self.voices[slot] = Some(v);
            }
            NoteEv::Off(key) => {
                for v in self.voices.iter_mut().flatten().filter(|v| v.key == key && v.on) {
                    if self.sustain { v.held = true } else { v.on = false }
                }
            }
            NoteEv::Sustain(down) => {
                self.sustain = down;
                if !down {
                    for v in self.voices.iter_mut().flatten().filter(|v| v.held) {
                        v.held = false;
                        v.on = false;
                    }
                }
            }
            NoteEv::Release => {
                self.sustain = false;
                for v in self.voices.iter_mut().flatten() {
                    v.on = false;
                    v.held = false;
                }
            }
            NoteEv::AllOff => self.voices = [None; 8],
        }
    }

    fn process(&mut self, p: &[f32; N_PARAMS], out: &mut [f32]) {
        out.fill(0.0);
        let sr = self.sr;
        let wave = p[0];
        let cutoff = 60.0 * 2f32.powf(p[1] * 8.4); // 60 Hz .. ~20 kHz
        let k = 2.0 - 1.9 * p[2]; // resonance
        let attack = 0.001 * 2000f32.powf(p[3]); // 1 ms .. 2 s
        let release = 0.01 * 400f32.powf(p[4]); // 10 ms .. 4 s
        let detune = 1.0 + 0.012 * p[5];
        let a_step = 1.0 / (attack * sr);
        let r_mul = (-1.0 / (release * sr)).exp();
        for slot in self.voices.iter_mut() {
            let Some(v) = slot else { continue };
            let f = 440.0 * 2f32.powf((v.key as f32 - 69.0) / 12.0);
            let dts = [f / sr, f * detune / sr];
            let fc = (cutoff * (1.0 + v.env * 0.5)).min(sr * 0.45);
            let g = (PI * fc / sr).tan();
            let (a1, a2) = (1.0 / (1.0 + g * (g + k)), g / (1.0 + g * (g + k)));
            let a3 = g * a2;
            for o in out.iter_mut() {
                let mut x = 0.0;
                for (ph, dt) in v.ph.iter_mut().zip(dts) {
                    // a falling saw: its fundamental is in phase with the square's,
                    // so blending the two never cancels the note itself
                    let saw = 1.0 - 2.0 * *ph + poly_blep(*ph, dt);
                    let sq = (if *ph < 0.5 { 1.0 } else { -1.0 }) + poly_blep(*ph, dt) - poly_blep((*ph + 0.5).fract(), dt);
                    x += saw * (1.0 - wave) + sq * wave;
                    *ph = (*ph + dt).fract();
                }
                // trapezoidal state-variable low-pass
                let v3 = x * 0.5 - v.ic2;
                let v1 = a1 * v.ic1 + a2 * v3;
                let v2 = v.ic2 + a2 * v.ic1 + a3 * v3;
                v.ic1 = 2.0 * v1 - v.ic1;
                v.ic2 = 2.0 * v2 - v.ic2;
                v.env = if v.on { (v.env + a_step).min(1.0) } else { v.env * r_mul };
                *o += v2 * v.env * v.vel * 0.6;
            }
            if !v.on && v.env < 1e-4 {
                *slot = None;
            }
        }
    }
}

// --- a VST3 instrument -------------------------------------------------------------------

pub struct VstInst {
    pub proc: crate::vst::VstProc,
    pending: [Option<(bool, u8, u8)>; 160],
    n_pending: usize,
    sustain: bool,
    held: [bool; 128],
    sounding: [bool; 128],
}

impl VstInst {
    fn queue(&mut self, e: (bool, u8, u8)) {
        if self.n_pending < self.pending.len() {
            self.pending[self.n_pending] = Some(e);
            self.n_pending += 1;
        }
    }
}

impl Instrument for VstInst {
    fn note(&mut self, ev: NoteEv) {
        // the pedal is kept here (VST3 has no plain CC events): held notes go off on release
        match ev {
            NoteEv::On(k, v) => {
                self.held[k as usize & 127] = false;
                self.sounding[k as usize & 127] = true;
                self.queue((true, k, v));
            }
            NoteEv::Off(k) if self.sustain => self.held[k as usize & 127] = true,
            NoteEv::Off(k) => {
                self.sounding[k as usize & 127] = false;
                self.queue((false, k, 0));
            }
            NoteEv::Release => {
                self.sustain = false;
                for k in 0..128u8 {
                    self.held[k as usize] = false;
                    if std::mem::take(&mut self.sounding[k as usize]) {
                        self.queue((false, k, 0));
                    }
                }
            }
            NoteEv::Sustain(down) => {
                self.sustain = down;
                if !down {
                    for k in 0..128u8 {
                        if std::mem::take(&mut self.held[k as usize]) {
                            self.queue((false, k, 0));
                        }
                    }
                }
            }
            NoteEv::AllOff => {
                self.sounding = [false; 128];
                for k in 0..128u8 {
                    self.queue((false, k, 0));
                }
            }
        }
    }

    fn set_sample_rate(&mut self, sr: f32) -> bool {
        self.proc.set_sample_rate(sr as f64);
        true
    }

    fn process(&mut self, _p: &[f32; N_PARAMS], out: &mut [f32]) {
        for i in 0..self.n_pending {
            if let Some((on, k, v)) = self.pending[i].take() {
                self.proc.note(on, k, v, 0);
            }
        }
        self.n_pending = 0;
        out.fill(0.0);
        let tempo = f64::from_bits(crate::fx::TEMPO_BITS.load(std::sync::atomic::Ordering::Relaxed));
        self.proc.process(out, 1.0, tempo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(inst: &mut dyn Instrument, p: &[f32; N_PARAMS], blocks: usize) -> Vec<f32> {
        let mut all = Vec::new();
        let mut buf = [0f32; 128];
        for _ in 0..blocks {
            inst.process(p, &mut buf);
            all.extend_from_slice(&buf);
        }
        all
    }
    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |a, v| a.max(v.abs()))
    }

    #[test]
    fn every_pad_has_a_sound_that_fades_out() {
        for i in 0..PADS.len() {
            let s = drum_sound(i, 48000.0);
            assert!(s.len() > 1000 && s.iter().all(|v| v.is_finite()), "pad {i}");
            assert!((peak(&s) - 0.9).abs() < 1e-3, "pad {i} normalised");
            assert!(peak(&s[s.len() - 200..]) < 0.05, "pad {} ({}) ends quietly", i, PADS[i].1);
        }
    }

    #[test]
    fn the_kit_plays_its_pads_and_chokes_the_open_hat() {
        let (mut kit, err) = DrumKit::new(48000.0, &[]);
        assert!(err.is_none());
        let p = DEFAULTS[DRUMS];
        assert_eq!(peak(&run(&mut kit, &p, 4)), 0.0, "silent until hit");
        kit.note(NoteEv::On(36, 127));
        assert!(peak(&run(&mut kit, &p, 4)) > 0.3, "kick");
        assert_eq!(DrumKit::pad_for(60), DrumKit::pad_for(44), "other notes wrap onto the pads");
        let (mut kit, _) = DrumKit::new(48000.0, &[]);
        kit.note(NoteEv::On(46, 127));
        run(&mut kit, &p, 2);
        kit.note(NoteEv::On(42, 1)); // a barely audible closed hat cuts the open one
        let after = run(&mut kit, &p, 40);
        assert!(peak(&after[after.len() - 512..]) < 1e-3, "open hat choked");
    }

    #[test]
    fn the_synth_plays_holds_with_the_pedal_and_releases() {
        let mut s = Synth::new(48000.0);
        let p = DEFAULTS[SYNTH];
        s.note(NoteEv::On(57, 100));
        assert!(peak(&run(&mut s, &p, 40)) > 0.05);
        s.note(NoteEv::Sustain(true));
        s.note(NoteEv::Off(57));
        let held = run(&mut s, &p, 200);
        assert!(peak(&held[held.len() - 512..]) > 0.02, "pedal holds the note");
        s.note(NoteEv::Sustain(false));
        let gone = run(&mut s, &p, 1500);
        assert!(peak(&gone[gone.len() - 512..]) < 1e-3, "released once the pedal comes up");
        assert!(s.voices.iter().all(|v| v.is_none()));
        // more notes than voices: the oldest is stolen, nothing breaks
        for k in 40..60 {
            s.note(NoteEv::On(k, 90));
        }
        assert!(run(&mut s, &p, 20).iter().all(|v| v.is_finite()));
    }
}
