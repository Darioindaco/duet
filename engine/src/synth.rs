//! The synth: a subtractive polysynth (16 voices).
//!
//! Per voice: two oscillators (each from saw through square, the second
//! detuned and transposable), a sub an octave down and noise; up to 7 unison
//! copies, detuned and spread across the stereo field; a filter (low-pass 24
//! or 12 dB, band-pass, high-pass) with its own envelope, key tracking and
//! velocity; an ADSR for the level. An LFO for vibrato, filter sweeps and
//! tremolo; glide; mono (legato) mode; drive.
//!
//! Parameters (all 0..1, by name; the first six are the old synth's, with
//! the same meaning, so old songs still load): see PARAMS in instr.rs.

use std::f32::consts::PI;

use crate::instr::{Instrument, NoteEv, N_PARAMS};

pub const VOICES: usize = 16;
const UNISON: usize = 7;

// where each knob is
const WAVE: usize = 0;
const CUTOFF: usize = 1;
const RES: usize = 2;
const ATTACK: usize = 3;
const RELEASE: usize = 4;
const DETUNE: usize = 5;
const DECAY: usize = 6;
const SUSTAIN: usize = 7;
const OSC2: usize = 8;
const OSC2_SEMI: usize = 9;
const OSC2_WAVE: usize = 10;
const SUB: usize = 11;
const NOISE: usize = 12;
const UNI: usize = 13;
const SPREAD: usize = 14;
const FILTER: usize = 15;
const ENV_AMT: usize = 16;
const F_ATTACK: usize = 17;
const F_DECAY: usize = 18;
const F_SUSTAIN: usize = 19;
const F_RELEASE: usize = 20;
const KEY_TRACK: usize = 21;
const LFO_RATE: usize = 22;
const LFO_PITCH: usize = 23;
const LFO_FILTER: usize = 24;
const LFO_AMP: usize = 25;
const GLIDE: usize = 26;
const MONO: usize = 27;
const VELOCITY: usize = 28;
const DRIVE: usize = 29;
const LEVEL: usize = 30;

pub const NAMES: [&str; 31] = [
    "wave", "cutoff", "resonance", "attack", "release", "detune", "decay", "sustain", "osc2", "osc2_semi", "osc2_wave",
    "sub", "noise", "unison", "spread", "filter", "env_amt", "f_attack", "f_decay", "f_sustain", "f_release", "key_track",
    "lfo_rate", "lfo_pitch", "lfo_filter", "lfo_amp", "glide", "mono", "velocity", "drive", "level",
];

pub const fn defaults() -> [f32; N_PARAMS] {
    let mut p = [0f32; N_PARAMS];
    p[WAVE] = 0.3;
    p[CUTOFF] = 0.55;
    p[RES] = 0.25;
    p[ATTACK] = 0.05;
    p[RELEASE] = 0.3;
    p[DETUNE] = 0.2;
    p[DECAY] = 0.5;
    p[SUSTAIN] = 1.0;
    p[OSC2] = 0.5;
    p[OSC2_SEMI] = 0.5;
    p[OSC2_WAVE] = 0.3;
    p[SPREAD] = 0.5;
    p[FILTER] = 0.0;
    p[ENV_AMT] = 0.5;
    p[F_DECAY] = 0.55;
    p[F_RELEASE] = 0.3;
    p[KEY_TRACK] = 0.3;
    p[LFO_RATE] = 0.45;
    p[VELOCITY] = 0.6;
    p[LEVEL] = 0.7;
    p
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

/// From saw (0) through to square (1), band-limited.
#[inline]
fn osc(ph: f32, dt: f32, wave: f32) -> f32 {
    // a falling saw: its fundamental is in phase with the square's, so
    // blending the two never cancels the note itself
    let saw = 1.0 - 2.0 * ph + poly_blep(ph, dt);
    let sq = (if ph < 0.5 { 1.0 } else { -1.0 }) + poly_blep(ph, dt) - poly_blep((ph + 0.5).fract(), dt);
    saw * (1.0 - wave) + sq * wave
}

#[derive(Clone, Copy, Default)]
struct Env {
    stage: u8, // 0 idle, 1 attack, 2 decay, 3 sustain, 4 release
    v: f32,
}

impl Env {
    fn gate(&mut self, on: bool) {
        if on {
            self.stage = 1;
        } else if self.stage != 0 {
            self.stage = 4;
        }
    }
    #[inline]
    fn tick(&mut self, a_step: f32, d_mul: f32, s: f32, r_mul: f32) -> f32 {
        match self.stage {
            1 => {
                self.v += a_step;
                if self.v >= 1.0 {
                    self.v = 1.0;
                    self.stage = 2;
                }
            }
            2 => {
                self.v = s + (self.v - s) * d_mul;
                if (self.v - s).abs() < 1e-4 {
                    self.stage = 3;
                }
            }
            3 => self.v = s,
            4 => {
                self.v *= r_mul;
                if self.v < 1e-3 { // -60 dB: the release time is up
                    self.v = 0.0;
                    self.stage = 0;
                }
            }
            _ => self.v = 0.0,
        }
        self.v
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SVoice {
    key: u8,
    on: bool,   // key still down (or held by the pedal)
    held: bool, // released while the pedal was down
    vel: f32,
    age: u64,
    /// where the pitch is (semitones), gliding to `target`
    pitch: f32,
    target: f32,
    ph: [f32; 2 * UNISON + 1],
    amp: Env,
    fenv: Env,
    /// two state-variable stages, per side: [ic1, ic2] each
    flt: [[f32; 4]; 2],
    noise: u32,
}

pub struct Synth {
    pub(crate) voices: [Option<SVoice>; VOICES],
    sr: f32,
    sustain: bool,
    clock: u64,
    lfo: f32,
    last_key: f32,
    /// mono mode: the keys held, newest last
    stack: [u8; 16],
    n_stack: usize,
    mono: bool,
    pending: Option<(u8, u8)>,
}

impl Synth {
    pub fn new(sr: f32) -> Self {
        Synth { voices: [None; VOICES], sr, sustain: false, clock: 0, lfo: 0.0, last_key: -1.0, stack: [0; 16], n_stack: 0, mono: false, pending: None }
    }

    fn start(&mut self, key: u8, vel: u8, glide: bool) {
        self.clock += 1;
        let from = if glide && self.last_key >= 0.0 { self.last_key } else { key as f32 };
        let mut v = SVoice {
            key, on: true, held: false, vel: vel as f32 / 127.0, age: self.clock, pitch: from, target: key as f32,
            ph: [0.0; 2 * UNISON + 1], amp: Env::default(), fenv: Env::default(), flt: [[0.0; 4]; 2],
            noise: (self.clock as u32).wrapping_mul(2_654_435_761) | 1,
        };
        // the unison copies start at different phases (or they'd sound as one);
        // the two oscillators of a copy together, so undetuned they're one sound
        for (i, ph) in v.ph.iter_mut().enumerate() {
            *ph = (((i / 2) as u32).wrapping_mul(0x9E37_79B9) >> 8) as f32 / 16_777_216.0;
        }
        // same key again, a free voice, or steal the oldest (from where it was: no click)
        let slot = self.voices.iter().position(|x| x.is_some_and(|x| x.key == key))
            .or_else(|| self.voices.iter().position(|x| x.is_none()))
            .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| self.voices[i].map_or(0, |x| x.age)).unwrap());
        if let Some(old) = self.voices[slot] {
            v.amp.v = old.amp.v;
            v.flt = old.flt;
        }
        v.amp.gate(true);
        v.fenv.gate(true);
        self.voices[slot] = Some(v);
        self.last_key = key as f32;
    }
}

impl Instrument for Synth {
    fn note(&mut self, ev: NoteEv) {
        match ev {
            NoteEv::On(key, vel) => {
                if self.mono {
                    // legato: the voice that's sounding moves to the new key
                    if self.n_stack < self.stack.len() {
                        self.stack[self.n_stack] = key;
                        self.n_stack += 1;
                    }
                    if let Some(v) = self.voices.iter_mut().flatten().find(|v| v.on) {
                        v.key = key;
                        v.target = key as f32;
                        v.vel = vel as f32 / 127.0;
                        self.last_key = key as f32;
                        return;
                    }
                }
                self.pending = Some((key, vel)); // started in process, which knows the glide
            }
            NoteEv::Off(key) => {
                if self.mono {
                    let n = self.n_stack;
                    if let Some(i) = self.stack[..n].iter().rposition(|k| *k == key) {
                        self.stack.copy_within(i + 1..n, i);
                        self.n_stack -= 1;
                    }
                    // back to the key still held, if any
                    if self.n_stack > 0 {
                        let back = self.stack[self.n_stack - 1];
                        if let Some(v) = self.voices.iter_mut().flatten().find(|v| v.on && v.key == key) {
                            v.key = back;
                            v.target = back as f32;
                        }
                        return;
                    }
                }
                for v in self.voices.iter_mut().flatten().filter(|v| v.key == key && v.on) {
                    if self.sustain {
                        v.held = true;
                    } else {
                        v.on = false;
                        v.amp.gate(false);
                        v.fenv.gate(false);
                    }
                }
            }
            NoteEv::Sustain(down) => {
                self.sustain = down;
                if !down {
                    for v in self.voices.iter_mut().flatten().filter(|v| v.held) {
                        v.held = false;
                        v.on = false;
                        v.amp.gate(false);
                        v.fenv.gate(false);
                    }
                }
            }
            NoteEv::Release => {
                self.sustain = false;
                self.n_stack = 0;
                for v in self.voices.iter_mut().flatten() {
                    v.on = false;
                    v.held = false;
                    v.amp.gate(false);
                    v.fenv.gate(false);
                }
            }
            NoteEv::AllOff => {
                self.voices = [None; VOICES];
                self.n_stack = 0;
            }
        }
    }

    fn process(&mut self, p: &[f32; N_PARAMS], out: &mut [f32], out_r: &mut [f32]) {
        out.fill(0.0);
        out_r.fill(0.0);
        let sr = self.sr;
        let was_mono = self.mono;
        self.mono = p[MONO] >= 0.5;
        if self.mono && !was_mono {
            // into mono: one voice from here on
            let keep = (0..VOICES).filter(|&i| self.voices[i].is_some_and(|v| v.on)).max_by_key(|&i| self.voices[i].unwrap().age);
            for i in 0..VOICES {
                if Some(i) != keep {
                    if let Some(v) = self.voices[i].as_mut() {
                        v.on = false;
                        v.amp.gate(false);
                        v.fenv.gate(false);
                    }
                }
            }
        }
        if let Some((key, vel)) = self.pending.take() {
            let glide = p[GLIDE] > 0.001;
            if self.mono {
                for v in self.voices.iter_mut().flatten() {
                    v.on = false;
                    v.amp.gate(false);
                    v.fenv.gate(false);
                }
                self.n_stack = 1;
                self.stack[0] = key;
            }
            self.start(key, vel, glide);
        }
        // the knobs, as numbers
        let t_attack = |x: f32| 0.001 * 2000f32.powf(x); // 1 ms .. 2 s
        let t_decay = |x: f32| 0.005 * 800f32.powf(x); // 5 ms .. 4 s (to -60 dB, or the sustain)
        let t_release = |x: f32| 0.02 * 500f32.powf(x); // 20 ms .. 10 s
        let (a_step, d_mul, s_lvl, r_mul) = (
            1.0 / (t_attack(p[ATTACK]) * sr),
            (-6.9 / (t_decay(p[DECAY]) * sr)).exp(),
            p[SUSTAIN],
            (-6.9 / (t_release(p[RELEASE]) * sr)).exp(),
        );
        let (fa_step, fd_mul, fs_lvl, fr_mul) = (
            1.0 / (t_attack(p[F_ATTACK]) * sr),
            (-6.9 / (t_decay(p[F_DECAY]) * sr)).exp(),
            p[F_SUSTAIN],
            (-6.9 / (t_release(p[F_RELEASE]) * sr)).exp(),
        );
        let n_uni = 1 + (p[UNI] * 6.0).round() as usize;
        let detune_c = p[DETUNE] * if n_uni > 1 { 40.0 } else { 20.0 }; // cents
        let semi2 = ((p[OSC2_SEMI] - 0.5) * 24.0).round();
        let (o2, sub, noise) = (p[OSC2], p[SUB], p[NOISE] * p[NOISE]);
        let spread = p[SPREAD];
        let ftype = ((p[FILTER] * 4.0) as usize).min(3);
        let k = 2.0 - 1.95 * p[RES].clamp(0.0, 1.0); // resonance (k = 1/Q)
        let base_cut = (60.0 * 2f32.powf(p[CUTOFF] * 8.4)).log2();
        let env_oct = (p[ENV_AMT] - 0.5) * 2.0 * 6.0;
        let vel_s = p[VELOCITY];
        let lfo_hz = 0.05 * 2f32.powf(p[LFO_RATE] * 8.0);
        let (lp, lf, la) = (p[LFO_PITCH] * p[LFO_PITCH] * 2.0, p[LFO_FILTER] * 3.0, p[LFO_AMP]);
        let glide_c = if p[GLIDE] > 0.001 { 1.0 - (-4.6 / (p[GLIDE] * p[GLIDE] * sr)).exp() } else { 1.0 };
        let drive = p[DRIVE];
        let level = p[LEVEL] * p[LEVEL] * 1.25 / (1.0 + 0.35 * (n_uni as f32 - 1.0)).sqrt();
        // unison copies: detune and pan, spread evenly
        let mut uni = [(0f32, 0.5f32, 0.5f32); UNISON];
        for (i, u) in uni.iter_mut().enumerate().take(n_uni) {
            let x = if n_uni == 1 { 0.0 } else { i as f32 / (n_uni - 1) as f32 * 2.0 - 1.0 };
            let pan = 0.5 + 0.5 * x * spread;
            *u = (2f32.powf(x * detune_c / 1200.0), (pan * PI / 2.0).cos(), (pan * PI / 2.0).sin());
        }
        // one oscillator pair alone: the second leans the other way (detune makes it wide)
        let pair_pan = 0.3 * spread * (p[DETUNE] * 5.0).min(1.0);
        let pair_det = if n_uni == 1 { 2f32.powf(detune_c / 1200.0) } else { 1.0 };
        let semi2_r = 2f32.powf(semi2 / 12.0);
        const CHUNK: usize = 16;
        let n = out.len();
        let mut lfo_ph = self.lfo;
        for slot in self.voices.iter_mut() {
            let Some(v) = slot else { continue };
            lfo_ph = self.lfo;
            let mut s0 = 0;
            while s0 < n {
                let s1 = (s0 + CHUNK).min(n);
                let lfo = (2.0 * PI * lfo_ph).sin();
                lfo_ph = (lfo_ph + lfo_hz * (s1 - s0) as f32 / sr).fract();
                // the filter for this stretch: cutoff, its envelope, the key, velocity, the LFO
                let vel_oct = vel_s * (v.vel - 0.7) * 2.0;
                let cut = base_cut + env_oct * v.fenv.v + p[KEY_TRACK] * (v.pitch - 60.0) / 12.0 + vel_oct + lf * lfo;
                let fc = 2f32.powf(cut).clamp(20.0, sr * 0.45);
                let g = (PI * fc / sr).tan();
                let a1 = 1.0 / (1.0 + g * (g + k));
                let (a2, a3) = (g * a1, g * g * a1);
                let k2 = std::f32::consts::SQRT_2;
                let b1 = 1.0 / (1.0 + g * (g + k2));
                let (b2, b3) = (g * b1, g * g * b1);
                // the pitch for this stretch too (glide and vibrato move it)
                let gc = 1.0 - (1.0 - glide_c).powi((s1 - s0) as i32);
                v.pitch += (v.target - v.pitch) * gc;
                let f = 440.0 * 2f32.powf((v.pitch + lp * lfo - 69.0) / 12.0);
                let f2 = f * semi2_r * pair_det;
                for s in s0..s1 {
                    let (mut l, mut r) = (0f32, 0f32);
                    for (u, &(det, gl, gr)) in uni.iter().enumerate().take(n_uni) {
                        let (d1, d2) = (f * det / sr, f2 * det / sr);
                        let x1 = osc(v.ph[2 * u], d1, p[WAVE]);
                        let x2 = osc(v.ph[2 * u + 1], d2, p[OSC2_WAVE]) * o2;
                        v.ph[2 * u] = (v.ph[2 * u] + d1).fract();
                        v.ph[2 * u + 1] = (v.ph[2 * u + 1] + d2).fract();
                        if n_uni == 1 {
                            l += x1 * (0.5 + pair_pan) + x2 * (0.5 - pair_pan);
                            r += x1 * (0.5 - pair_pan) + x2 * (0.5 + pair_pan);
                        } else {
                            l += (x1 + x2) * gl;
                            r += (x1 + x2) * gr;
                        }
                    }
                    if n_uni > 1 {
                        let norm = 1.0 / (n_uni as f32).sqrt();
                        l *= norm;
                        r *= norm;
                    }
                    // the sub (an octave down) and noise, in the middle
                    let sp = &mut v.ph[2 * UNISON];
                    let subx = (2.0 * PI * *sp).sin() * sub;
                    *sp = (*sp + f * 0.5 / sr).fract();
                    v.noise ^= v.noise << 13;
                    v.noise ^= v.noise >> 17;
                    v.noise ^= v.noise << 5;
                    let nz = ((v.noise as f32 / u32::MAX as f32) * 2.0 - 1.0) * noise;
                    let amp = v.amp.tick(a_step, d_mul, s_lvl, r_mul);
                    v.fenv.tick(fa_step, fd_mul, fs_lvl, fr_mul);
                    let vel_g = 1.0 - vel_s + vel_s * v.vel;
                    let trem = 1.0 - la * 0.5 * (1.0 + lfo);
                    let gain = amp * vel_g * trem * level;
                    for (side, (x, dst)) in [l + subx + nz, r + subx + nz].into_iter().zip([&mut out[s], &mut out_r[s]]).enumerate() {
                        let st = &mut v.flt[side];
                        // first stage (with the resonance): trapezoidal state-variable filter
                        let v3 = x - st[1];
                        let v1 = a1 * st[0] + a2 * v3;
                        let v2 = st[1] + a2 * st[0] + a3 * v3;
                        st[0] = 2.0 * v1 - st[0];
                        st[1] = 2.0 * v2 - st[1];
                        let y = match ftype {
                            0 => {
                                // and a second, plain low-pass: 24 dB/octave
                                let w3 = v2 - st[3];
                                let w1 = b1 * st[2] + b2 * w3;
                                let w2 = st[3] + b2 * st[2] + b3 * w3;
                                st[2] = 2.0 * w1 - st[2];
                                st[3] = 2.0 * w2 - st[3];
                                w2
                            }
                            1 => v2,
                            2 => v1 * k,
                            _ => x - k * v1 - v2,
                        };
                        let y = if drive > 0.001 { (y * (1.0 + drive * 6.0)).tanh() / (1.0 + drive * 2.0) } else { y };
                        *dst += y * gain;
                    }
                }
                s0 = s1;
            }
            if v.amp.stage == 0 {
                *slot = None;
            }
        }
        self.lfo = (self.lfo + lfo_hz * n as f32 / sr).fract();
        let _ = lfo_ph;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Synth, p: &[f32; N_PARAMS], blocks: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (Vec::new(), Vec::new());
        let (mut bl, mut br) = ([0f32; 128], [0f32; 128]);
        for _ in 0..blocks {
            s.process(p, &mut bl, &mut br);
            l.extend_from_slice(&bl);
            r.extend_from_slice(&br);
        }
        (l, r)
    }
    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |a, v| a.max(v.abs()))
    }
    fn pitch(x: &[f32]) -> f32 {
        // through a low-pass first (a falling saw crosses zero more than once a period otherwise)
        let mut y = 0f32;
        let lp: Vec<f32> = x.iter().map(|v| { y += 0.05 * (v - y); y }).collect();
        let c = lp.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        c as f32 / (x.len() as f32 / 48000.0)
    }

    #[test]
    fn every_knob_makes_a_sound_that_stays_sane() {
        // each knob at 0 and at 1 (the rest at their defaults): a note, then let go
        for k in 0..NAMES.len() {
            for x in [0.0, 1.0] {
                let mut p = defaults();
                p[k] = x;
                let mut s = Synth::new(48000.0);
                s.note(NoteEv::On(57, 110));
                let (l, r) = run(&mut s, &p, 150);
                assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 4.0), "{} = {x}", NAMES[k]);
                if !(k == LEVEL && x == 0.0) && !(k == CUTOFF && x == 0.0) && !(k == SUSTAIN && x == 0.0) && !(k == ATTACK && x == 1.0) {
                    assert!(peak(&l) > 0.01, "{} = {x}: silent", NAMES[k]);
                }
                s.note(NoteEv::Off(57));
                run(&mut s, &p, 4500);
                assert!(s.voices.iter().all(|v| v.is_none()), "{} = {x}: the note ends", NAMES[k]);
            }
        }
    }

    #[test]
    fn osc2_transposes_and_glide_slides() {
        let mut p = defaults();
        p[CUTOFF] = 1.0;
        p[OSC2] = 0.0;
        p[DETUNE] = 0.0;
        let mut s = Synth::new(48000.0);
        s.note(NoteEv::On(69, 100));
        let (l, _) = run(&mut s, &p, 100);
        assert!((pitch(&l[4800..]) - 440.0).abs() < 8.0, "{}", pitch(&l[4800..]));
        // glide from A4 to A5 in mono: halfway through it's in between
        p[MONO] = 1.0;
        p[GLIDE] = 0.5; // 0.25 s
        let mut s = Synth::new(48000.0);
        s.note(NoteEv::On(69, 100));
        run(&mut s, &p, 60);
        s.note(NoteEv::On(81, 100));
        let (l, _) = run(&mut s, &p, 300);
        let early = pitch(&l[960..4800]);
        let late = pitch(&l[l.len() - 9600..]);
        assert!(early > 450.0 && early < 860.0, "{early}");
        assert!((late - 880.0).abs() < 15.0, "{late}");
        // legato: letting go of the top goes back to the held one
        s.note(NoteEv::Off(81));
        let (l, _) = run(&mut s, &p, 300);
        assert!((pitch(&l[l.len() - 9600..]) - 440.0).abs() < 10.0);
    }

    #[test]
    fn unison_is_wide_and_one_voice_isnt() {
        let mut p = defaults();
        p[DETUNE] = 0.0;
        let mut s = Synth::new(48000.0);
        s.note(NoteEv::On(57, 100));
        let (l, r) = run(&mut s, &p, 100);
        assert!(l.iter().zip(&r).all(|(a, b)| (a - b).abs() < 1e-4), "no detune: centred");
        p[UNI] = 1.0;
        p[DETUNE] = 0.6;
        p[SPREAD] = 1.0;
        let mut s = Synth::new(48000.0);
        s.note(NoteEv::On(57, 100));
        let (l, r) = run(&mut s, &p, 100);
        let diff = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(diff > 0.05, "{diff}");
    }

    #[test]
    #[ignore] // a measurement: cargo test --release -- --ignored worst_case --nocapture
    fn worst_case_cpu() {
        let mut p = defaults();
        p[UNI] = 1.0;
        p[OSC2] = 1.0;
        p[SUB] = 1.0;
        p[NOISE] = 0.5;
        p[LFO_PITCH] = 0.3;
        p[LFO_FILTER] = 0.5;
        let mut s = Synth::new(48000.0);
        for k in 0..16 {
            s.note(NoteEv::On(40 + k * 2, 100));
        }
        let t = std::time::Instant::now();
        run(&mut s, &p, 375); // 1 s
        println!("16 voices x 7 unison: {:.1}% of one core", t.elapsed().as_secs_f64() * 100.0);
    }
}
