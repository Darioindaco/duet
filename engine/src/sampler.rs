//! The sampler: one sample (mono or stereo) played at each note's pitch, like Logic's or FL's.
//!
//! - where it plays: start and end, forwards or reversed; a loop inside that (with a crossfade,
//!   so the seam doesn't click) that keeps going while the note sounds, release included;
//! - a one-shot mode for drum hits (plays to the end whatever the note's length);
//! - the level's envelope (attack, decay, sustain, release) and velocity;
//! - a low-pass filter with its own envelope, key tracking and velocity;
//! - an LFO for vibrato, filter sweeps and tremolo; glide, mono (legato); pan; the pitch wheel.
//!
//! The first six knobs are the old sampler's, with the same meaning and the same curves, and
//! the new ones start where they change nothing: an old song sounds as it did.
//!
//! Realtime: no allocation after `new` (the voices' room is reserved there).

use std::f32::consts::PI;

use crate::instr::{Instrument, NoteEv, Zone, MAX_ZONES, N_PARAMS};

// where each knob is (by name in the song: NAMES)
const LEVEL: usize = 0;
const TUNE: usize = 1;
const ATTACK: usize = 2;
const RELEASE: usize = 3;
const ROOT: usize = 4;
const ONESHOT: usize = 5;
const START: usize = 6;
const END: usize = 7;
const LOOP: usize = 8;
const LOOP_START: usize = 9;
const LOOP_END: usize = 10;
const XFADE: usize = 11;
const REVERSE: usize = 12;
const DECAY: usize = 13;
const SUSTAIN: usize = 14;
const CUTOFF: usize = 15;
const RES: usize = 16;
const ENV_AMT: usize = 17;
const F_ATTACK: usize = 18;
const F_DECAY: usize = 19;
const F_SUSTAIN: usize = 20;
const F_RELEASE: usize = 21;
const LFO_RATE: usize = 22;
const LFO_PITCH: usize = 23;
const LFO_FILTER: usize = 24;
const LFO_AMP: usize = 25;
const GLIDE: usize = 26;
const MONO: usize = 27;
const VELOCITY: usize = 28;
const VEL_FILTER: usize = 29;
const PAN: usize = 30;
const KEY_TRACK: usize = 31;

pub const NAMES: [&str; 32] = [
    "level", "tune", "attack", "release", "root", "oneshot", "start", "end", "loop", "loop_start", "loop_end", "xfade",
    "reverse", "decay", "sustain", "cutoff", "resonance", "env_amt", "f_attack", "f_decay", "f_sustain", "f_release",
    "lfo_rate", "lfo_pitch", "lfo_filter", "lfo_amp", "glide", "mono", "velocity", "vel_filter", "pan", "key_track",
];

pub const fn defaults() -> [f32; N_PARAMS] {
    let mut p = [0f32; N_PARAMS];
    p[LEVEL] = 0.8;
    p[TUNE] = 0.5;
    p[RELEASE] = 0.15;
    p[ROOT] = 0.5;
    p[END] = 1.0;
    p[LOOP_END] = 1.0;
    p[XFADE] = 0.1;
    p[DECAY] = 0.5;
    p[SUSTAIN] = 1.0;
    p[CUTOFF] = 1.0;
    p[RES] = 0.1;
    p[ENV_AMT] = 0.5;
    p[F_DECAY] = 0.5;
    p[F_SUSTAIN] = 1.0;
    p[F_RELEASE] = 0.3;
    p[LFO_RATE] = 0.45;
    p[VELOCITY] = 1.0;
    p[PAN] = 0.5;
    p
}

pub const VOICES: usize = 16;

#[derive(Clone, Copy)]
struct Voice {
    key: u8,
    vel: f32,
    /// where in the sample, counted from where it starts playing (reversed: from the end)
    pos: f64,
    /// semitones, gliding to `target`
    pitch: f32,
    target: f32,
    on: bool,
    held: bool,
    amp: f32,
    /// 0 idle, 1 attack, 2 decay/sustain, 3 release
    stage: u8,
    fenv: f32,
    fstage: u8,
    flt: [[f32; 2]; 2],
    done: bool,
    /// just started: it glides from the last note's pitch only if there's a glide
    fresh: bool,
    /// which sample, and its zone's root (None: the root knob)
    smp: u8,
    root: Option<f32>,
}

/// One sample, both sides (r: None for mono).
struct Smp {
    l: Vec<f32>,
    r: Option<Vec<f32>>,
}

pub struct Sampler {
    smps: Vec<Option<Smp>>,
    zones: Vec<Zone>,
    /// per zone: the first zone with the same keys and velocities (its round-robin group), and
    /// each group's turn
    group: Vec<u8>,
    turn: [u32; MAX_ZONES],
    voices: Vec<Voice>,
    sr: f32,
    sustain: bool,
    bend: f32,
    lfo: f32,
    /// the last note's pitch (where a glide starts)
    last: Option<f32>,
}

impl Sampler {
    /// The samples (pads) and where they play. A sample that can't be read is left out and said.
    pub fn new(sr: f32, pads: &[Option<String>], zones: &[Zone]) -> (Sampler, Option<String>) {
        let mut err = None;
        let mut smps: Vec<Option<Smp>> = Vec::with_capacity(pads.len());
        for path in pads.iter().take(MAX_ZONES) {
            let smp = path.as_ref().and_then(|path| match crate::decode::read_audio(std::path::Path::new(path)) {
                Ok(a) => Some(Smp {
                    l: crate::daw::resample(&a.samples, a.sr, sr as u32),
                    r: a.right.as_ref().map(|r| crate::daw::resample(r, a.sr, sr as u32)),
                }),
                Err(e) => {
                    err.get_or_insert(e);
                    None
                }
            });
            smps.push(smp);
        }
        (Sampler::build(sr, smps, zones), err)
    }

    fn build(sr: f32, smps: Vec<Option<Smp>>, zones: &[Zone]) -> Sampler {
        let zones: Vec<Zone> = zones.iter().copied().filter(|z| (z.pad as usize) < smps.len()).take(MAX_ZONES).collect();
        let group = (0..zones.len())
            .map(|i| {
                let z = zones[i];
                zones.iter().position(|y| (y.lo, y.hi, y.vlo, y.vhi) == (z.lo, z.hi, z.vlo, z.vhi)).unwrap_or(i) as u8
            })
            .collect();
        Sampler { smps, zones, group, turn: [0; MAX_ZONES], voices: Vec::with_capacity(VOICES), sr, sustain: false, bend: 0.0, lfo: 0.0, last: None }
    }

    /// From a sample in memory (tests).
    #[cfg(test)]
    pub fn from_samples(sr: f32, l: Vec<f32>, r: Option<Vec<f32>>) -> Sampler {
        Sampler::build(sr, vec![Some(Smp { l, r })], &[])
    }

    #[cfg(test)]
    fn from_zones(sr: f32, smps: Vec<Vec<f32>>, zones: &[Zone]) -> Sampler {
        Sampler::build(sr, smps.into_iter().map(|l| Some(Smp { l, r: None })).collect(), zones)
    }

    fn start(&mut self, key: u8, vel: u8, smp: u8, root: Option<f32>) {
        if self.voices.len() >= VOICES {
            self.voices.remove(0); // (capacity is reserved: no allocation)
        }
        let mut v = Voice {
            key,
            vel: vel as f32 / 127.0,
            pos: 0.0,
            pitch: key as f32,
            target: key as f32,
            on: true,
            held: false,
            amp: 0.0,
            stage: 1,
            fenv: 0.0,
            fstage: 1,
            flt: [[0.0; 2]; 2],
            done: false,
            fresh: true,
            smp,
            root,
        };
        // (mono and glide are knobs: sorted out in process, with the pitch it glides from)
        if let Some(from) = self.last {
            v.pitch = from; // process puts it right back when there's no glide
        }
        self.voices.push(v);
    }

    fn release(v: &mut Voice) {
        v.on = false;
        v.held = false;
        v.stage = 3;
        v.fstage = 3;
    }
}

/// Where a voice plays: from `a` to `b` (frames of the sample), looping from `lb` back to `la`
/// once it gets there, with `xf` frames of crossfade before `lb`. All counted from where it starts
/// (reversed: from the end), so the same sums work both ways.
#[derive(Clone, Copy)]
struct Region {
    a: usize,
    b: usize,
    rev: bool,
    looping: bool,
    la: f64,
    lb: f64,
    xf: f64,
}

impl Region {
    fn of(p: &[f32; N_PARAMS], n: usize) -> Region {
        let nf = n as f32;
        let mut a = (p[START].clamp(0.0, 1.0) * nf) as usize;
        let mut b = (p[END].clamp(0.0, 1.0) * nf) as usize;
        if b < a {
            std::mem::swap(&mut a, &mut b);
        }
        b = b.min(n);
        if b < a + 2 {
            a = b.saturating_sub(2);
        }
        let rev = p[REVERSE] >= 0.5;
        let len = (b - a) as f64;
        // the loop, as fractions of the whole sample, inside start..end
        let (mut ls, mut le) = (p[LOOP_START].clamp(0.0, 1.0) as f64 * n as f64, p[LOOP_END].clamp(0.0, 1.0) as f64 * n as f64);
        if le < ls {
            std::mem::swap(&mut ls, &mut le);
        }
        let (ls, le) = (ls.clamp(a as f64, b as f64), le.clamp(a as f64, b as f64));
        // into "from where it starts"
        let (la, lb) = if rev { (b as f64 - le, b as f64 - ls) } else { (ls - a as f64, le - a as f64) };
        let looping = p[LOOP] >= 0.5 && p[ONESHOT] < 0.5 && lb - la >= 16.0;
        // the crossfade: up to half the loop, and no more than what there is before the loop
        let xf = if looping { (p[XFADE].clamp(0.0, 1.0) as f64 * 0.5 * (lb - la)).min(la).max(0.0) } else { 0.0 };
        Region { a, b, rev, looping, la, lb: lb.min(len), xf }
    }
    #[inline]
    fn len(&self) -> f64 {
        (self.b - self.a) as f64
    }
}

#[inline]
fn read(buf: &[f32], reg: &Region, q: f64) -> f32 {
    // q: from where it starts; the frame in the sample, between two (linear)
    let x = if reg.rev { reg.b as f64 - 1.0 - q } else { reg.a as f64 + q };
    if x < 0.0 {
        return 0.0;
    }
    let i = x as usize;
    let t = (x - i as f64) as f32;
    let s0 = buf.get(i).copied().unwrap_or(0.0);
    let s1 = buf.get(i + 1).copied().unwrap_or(s0);
    s0 + (s1 - s0) * t
}

/// A sample, at q, with the loop's crossfade: before the loop's end, what's after its start
/// comes in (equal power), so the jump back is to the same sound.
#[inline]
fn tap(buf: &[f32], reg: &Region, q: f64) -> f32 {
    if reg.looping && reg.xf > 0.0 && q > reg.lb - reg.xf {
        let w = ((q - (reg.lb - reg.xf)) / reg.xf).clamp(0.0, 1.0) as f32;
        let back = q - (reg.lb - reg.la);
        read(buf, reg, q) * (1.0 - w).sqrt() + read(buf, reg, back) * w.sqrt()
    } else {
        read(buf, reg, q)
    }
}

impl Instrument for Sampler {
    fn note(&mut self, ev: NoteEv) {
        match ev {
            NoteEv::On(key, vel) => {
                if self.zones.is_empty() {
                    self.start(key, vel, 0, None);
                } else {
                    // every zone this key and velocity are in; of zones with the same range, the
                    // one whose turn it is
                    let mut done = [false; MAX_ZONES];
                    for i in 0..self.zones.len() {
                        let z = self.zones[i];
                        let g = self.group[i] as usize;
                        if done[g] || key < z.lo || key > z.hi || vel < z.vlo || vel > z.vhi {
                            continue;
                        }
                        done[g] = true;
                        let n = self.group.iter().filter(|&&x| x as usize == g).count() as u32;
                        let pick = (self.turn[g] % n.max(1)) as usize;
                        self.turn[g] = self.turn[g].wrapping_add(1);
                        let zi = (0..self.zones.len()).filter(|&j| self.group[j] as usize == g).nth(pick).unwrap_or(i);
                        let zz = self.zones[zi];
                        self.start(key, vel, zz.pad, Some(zz.root as f32));
                    }
                }
                self.last = Some(key as f32);
            }
            NoteEv::Off(key) => {
                for v in self.voices.iter_mut().filter(|v| v.key == key && v.on) {
                    if self.sustain {
                        v.held = true;
                    } else {
                        Sampler::release(v);
                    }
                }
            }
            NoteEv::Sustain(down) => {
                self.sustain = down;
                if !down {
                    for v in self.voices.iter_mut().filter(|v| v.held) {
                        Sampler::release(v);
                    }
                }
            }
            NoteEv::Release => {
                self.sustain = false;
                for v in self.voices.iter_mut() {
                    Sampler::release(v);
                }
            }
            NoteEv::AllOff => self.voices.clear(),
            NoteEv::Bend(b) => self.bend = b.clamp(-1.0, 1.0) * 2.0,
            NoteEv::Cc(..) => {}
        }
    }

    fn set_sample_rate(&mut self, _sr: f32) -> bool {
        false // (the sample is resampled when it's loaded: build it again)
    }

    fn process(&mut self, p: &[f32; N_PARAMS], out_l: &mut [f32], out_r: &mut [f32]) {
        out_l.fill(0.0);
        out_r.fill(0.0);
        if self.smps.iter().all(|x| x.as_ref().is_none_or(|x| x.l.len() < 2)) {
            self.voices.clear();
            return;
        }
        let sr = self.sr;
        // the old sampler's knobs, as they were
        let level = p[LEVEL] * p[LEVEL] * 1.6;
        let tune = (p[TUNE] - 0.5) * 24.0; // ±12 semitones
        let attack = 0.0005 + 0.5 * p[ATTACK] * p[ATTACK]; // up to 0.5 s
        let release = 0.005 + 3.0 * p[RELEASE] * p[RELEASE]; // up to 3 s
        let root = 60.0 + ((p[ROOT] - 0.5) * 72.0).round(); // C1 .. C7
        let oneshot = p[ONESHOT] >= 0.5;
        let a_step = 1.0 / (attack * sr);
        let r_mul = (-1.0 / (release * sr)).exp();
        // the new ones
        let t_decay = |x: f32| 0.005 * 800f32.powf(x); // 5 ms .. 4 s
        let d_mul = (-6.9 / (t_decay(p[DECAY]) * sr)).exp();
        let s_lvl = p[SUSTAIN].clamp(0.0, 1.0);
        let fa_step = 1.0 / ((0.001 * 2000f32.powf(p[F_ATTACK])) * sr);
        let fd_mul = (-6.9 / (t_decay(p[F_DECAY]) * sr)).exp();
        let fs_lvl = p[F_SUSTAIN].clamp(0.0, 1.0);
        let fr_mul = (-6.9 / ((0.02 * 500f32.powf(p[F_RELEASE])) * sr)).exp();
        let env_oct = (p[ENV_AMT] - 0.5) * 2.0 * 6.0;
        let vel_oct = p[VEL_FILTER] * 4.0;
        let lfo_hz = 0.05 * 2f32.powf(p[LFO_RATE] * 8.0);
        let (lp, lf, la) = (p[LFO_PITCH] * p[LFO_PITCH] * 2.0, p[LFO_FILTER] * 3.0, p[LFO_AMP]);
        // the filter is out of the way when it's open and nothing moves it
        let filtering = p[CUTOFF] < 0.999 || env_oct < -1e-3 || lf > 1e-3 || vel_oct > 1e-3 || p[KEY_TRACK] > 1e-3;
        let base_cut = (60.0 * 2f32.powf(p[CUTOFF] * 8.4)).log2();
        let k = 2.0 - 1.95 * p[RES].clamp(0.0, 1.0);
        let glide = p[GLIDE];
        let glide_c = if glide > 0.001 { 1.0 - (-4.6 / (glide * glide * sr)).exp() } else { 1.0 };
        let vel_s = p[VELOCITY].clamp(0.0, 1.0);
        let pan = p[PAN].clamp(0.0, 1.0);
        let (pl, pr) = if (pan - 0.5).abs() < 1e-4 { (1.0, 1.0) } else { ((pan * PI / 2.0).cos() * 2f32.sqrt(), (pan * PI / 2.0).sin() * 2f32.sqrt()) };
        // mono: only the newest voice sounds (it glides from where the last one was)
        if p[MONO] >= 0.5 && self.voices.len() > 1 {
            let last = self.voices.len() - 1;
            let legato = self.voices[..last].iter().any(|v| v.on);
            let from = self.voices[last - 1].pitch;
            let pos = self.voices[last - 1].pos;
            for v in self.voices[..last].iter_mut() {
                v.done = true;
            }
            let nv = &mut self.voices[last];
            if legato && nv.stage == 1 && nv.amp == 0.0 {
                // legato: no new attack, the sample goes on where it was
                nv.pitch = from;
                nv.pos = pos;
                nv.amp = 1.0;
                nv.stage = 2;
                nv.fenv = 1.0;
                nv.fstage = 2;
            }
            self.voices.retain(|v| !v.done);
        }
        let n_out = out_l.len();
        const CHUNK: usize = 16;
        let smps = &self.smps;
        for v in self.voices.iter_mut() {
            if std::mem::take(&mut v.fresh) && glide <= 0.001 {
                v.pitch = v.target;
            }
            // its sample, where it plays in it, and its root (its zone's, else the knob's)
            let Some(smp) = smps.get(v.smp as usize).and_then(|x| x.as_ref()).filter(|x| x.l.len() >= 2) else {
                v.done = true;
                continue;
            };
            let reg = Region::of(p, smp.l.len());
            let len = reg.len();
            let root = v.root.unwrap_or(root);
            let right = smp.r.as_deref();
            let mut lfo_ph = self.lfo;
            let mut s0 = 0;
            while s0 < n_out {
                let s1 = (s0 + CHUNK).min(n_out);
                let lfo = (2.0 * PI * lfo_ph).sin();
                lfo_ph = (lfo_ph + lfo_hz * (s1 - s0) as f32 / sr).fract();
                let gc = 1.0 - (1.0 - glide_c).powi((s1 - s0) as i32);
                v.pitch += (v.target - v.pitch) * gc;
                let step = 2f64.powf(((v.pitch - root + tune + self.bend + lp * lfo) / 12.0) as f64);
                // this stretch's filter
                let (a1, a2, a3) = if filtering {
                    let cut = base_cut + env_oct * v.fenv + p[KEY_TRACK] * (v.pitch - root) / 12.0 + vel_oct * (v.vel - 0.7) + lf * lfo;
                    let fc = 2f32.powf(cut).clamp(20.0, sr * 0.45);
                    let g = (PI * fc / sr).tan();
                    let a1 = 1.0 / (1.0 + g * (g + k));
                    (a1, g * a1, g * g * a1)
                } else {
                    (0.0, 0.0, 0.0)
                };
                let trem = 1.0 - la * 0.5 * (1.0 + lfo);
                let vel_g = 1.0 - vel_s + vel_s * v.vel * v.vel;
                for s in s0..s1 {
                    if v.done {
                        break;
                    }
                    // past the end (or the loop's end): stop, or round again
                    if reg.looping && v.pos >= reg.lb {
                        v.pos -= reg.lb - reg.la;
                    }
                    if v.pos + 1.0 >= len {
                        v.done = true;
                        break;
                    }
                    let mut xl = tap(&smp.l, &reg, v.pos);
                    let mut xr = match right {
                        Some(r) => tap(r, &reg, v.pos),
                        None => xl,
                    };
                    // the level's envelope (the old sampler's attack and release, a decay to the sustain)
                    let playing = v.on || oneshot;
                    match v.stage {
                        1 => {
                            v.amp += a_step;
                            if v.amp >= 1.0 {
                                v.amp = 1.0;
                                v.stage = 2;
                            }
                        }
                        2 => v.amp = s_lvl + (v.amp - s_lvl) * d_mul,
                        _ => {}
                    }
                    if !playing {
                        v.stage = 3;
                    }
                    if v.stage == 3 {
                        if oneshot {
                            v.stage = 2; // a one-shot plays to the end
                        } else {
                            v.amp *= r_mul;
                            if v.amp < 1e-4 {
                                v.done = true;
                                break;
                            }
                        }
                    }
                    if filtering {
                        match v.fstage {
                            1 => {
                                v.fenv += fa_step;
                                if v.fenv >= 1.0 {
                                    v.fenv = 1.0;
                                    v.fstage = 2;
                                }
                            }
                            2 => v.fenv = fs_lvl + (v.fenv - fs_lvl) * fd_mul,
                            _ => v.fenv *= fr_mul,
                        }
                        for (side, x) in [&mut xl, &mut xr].into_iter().enumerate() {
                            let st = &mut v.flt[side];
                            let v3 = *x - st[1];
                            let v1 = a1 * st[0] + a2 * v3;
                            let v2 = st[1] + a2 * st[0] + a3 * v3;
                            st[0] = 2.0 * v1 - st[0];
                            st[1] = 2.0 * v2 - st[1];
                            *x = v2;
                        }
                    }
                    let gn = level * v.amp * vel_g * trem;
                    out_l[s] += xl * gn * pl;
                    out_r[s] += xr * gn * pr;
                    v.pos += step;
                }
                s0 = s1;
            }
        }
        self.lfo = (self.lfo + lfo_hz * n_out as f32 / sr).fract();
        self.voices.retain(|v| !v.done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Sampler, p: &[f32; N_PARAMS], frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            s.process(p, cl, cr);
        }
        (l, r)
    }

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32 / n as f32).collect()
    }

    #[test]
    fn plays_at_the_root_as_it_is_and_an_octave_up_twice_as_fast() {
        let sr = 48000.0;
        let sample: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut p = defaults();
        p[ONESHOT] = 1.0;
        let mut s = Sampler::from_samples(sr, sample.clone(), None);
        s.note(NoteEv::On(60, 127));
        let (l, _) = run(&mut s, &p, 6000);
        let lv = p[LEVEL] * p[LEVEL] * 1.6;
        // past the attack, it's the sample, at the level
        for i in [1000, 2000, 4000] {
            assert!((l[i] - sample[i] * lv).abs() < 1e-3, "{i}: {} vs {}", l[i], sample[i] * lv);
        }
        assert!(l[4810..].iter().all(|x| x.abs() < 1e-6), "stops at the end");
        let mut s = Sampler::from_samples(sr, sample, None);
        s.note(NoteEv::On(72, 127));
        let (l, _) = run(&mut s, &p, 6000);
        let last = l.iter().rposition(|x| x.abs() > 1e-6).unwrap();
        assert!((2380..2410).contains(&last), "an octave up: half as long ({last})");
    }

    #[test]
    fn start_end_reverse() {
        let sr = 48000.0;
        let mut p = defaults();
        p[ONESHOT] = 1.0;
        p[ATTACK] = 0.0;
        p[START] = 0.25;
        p[END] = 0.75;
        let lv = p[LEVEL] * p[LEVEL] * 1.6;
        let mut s = Sampler::from_samples(sr, ramp(4000), None);
        s.note(NoteEv::On(60, 127));
        let (l, _) = run(&mut s, &p, 3000);
        assert!((l[100] / lv - 0.275).abs() < 0.01, "starts a quarter in: {}", l[100] / lv);
        let last = l.iter().rposition(|x| x.abs() > 1e-6).unwrap();
        assert!((1990..2001).contains(&last), "plays half of it ({last})");
        p[REVERSE] = 1.0;
        let mut s = Sampler::from_samples(sr, ramp(4000), None);
        s.note(NoteEv::On(60, 127));
        let (l, _) = run(&mut s, &p, 3000);
        assert!((l[100] / lv - 0.725).abs() < 0.01, "reversed: from three quarters down: {}", l[100] / lv);
        assert!(l[1000] < l[100], "going down");
    }

    #[test]
    fn a_loop_keeps_going_while_the_note_is_held_and_the_seam_is_smooth() {
        let sr = 48000.0;
        // a sine whose loop points don't fall on the same phase: without the crossfade, a click
        let sample: Vec<f32> = (0..9600).map(|i| (i as f32 * 2.0 * PI * 220.0 / sr).sin() * 0.5).collect();
        let mut p = defaults();
        p[LOOP] = 1.0;
        p[LOOP_START] = 0.3;
        p[LOOP_END] = 0.71;
        p[XFADE] = 0.5;
        let mut s = Sampler::from_samples(sr, sample, None);
        s.note(NoteEv::On(60, 100));
        let (l, _) = run(&mut s, &p, 48000);
        // still sounding a second in (the sample is 0.2 s)
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        assert!(rms(&l[40000..48000]) > 0.1, "loops: {}", rms(&l[40000..48000]));
        // no jump bigger than a 220 Hz sine moves in one frame (plus a little)
        let max_step = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        let sine_step = 0.5 * 2.0 * PI * 220.0 / sr * (p[LEVEL] * p[LEVEL] * 1.6);
        assert!(max_step < sine_step * 2.5, "a seam: {max_step} vs {sine_step}");
        // let go: the release ends it
        s.note(NoteEv::Off(60));
        let (l, _) = run(&mut s, &p, 48000 * 2);
        assert!(rms(&l[48000 * 2 - 4800..]) < 1e-4);
        assert!(s.voices.is_empty());
    }

    #[test]
    fn the_filter_and_the_envelope() {
        let sr = 48000.0;
        // noise: the filter takes the top off
        let mut x = 1u32;
        let noise: Vec<f32> = (0..48000).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; (x as f32 / u32::MAX as f32) * 2.0 - 1.0 }).collect();
        let hf = |y: &[f32]| y.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>() / y.len() as f32;
        let mut p = defaults();
        let mut s = Sampler::from_samples(sr, noise.clone(), None);
        s.note(NoteEv::On(60, 127));
        let (open, _) = run(&mut s, &p, 24000);
        p[CUTOFF] = 0.35;
        let mut s = Sampler::from_samples(sr, noise.clone(), None);
        s.note(NoteEv::On(60, 127));
        let (shut, _) = run(&mut s, &p, 24000);
        assert!(hf(&shut[2000..]) < 0.05 * hf(&open[2000..]), "{} vs {}", hf(&shut[2000..]), hf(&open[2000..]));
        // decay to half: quieter later than early
        let mut p = defaults();
        p[DECAY] = 0.2;
        p[SUSTAIN] = 0.25;
        let mut s = Sampler::from_samples(sr, noise, None);
        s.note(NoteEv::On(60, 127));
        let (l, _) = run(&mut s, &p, 24000);
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let (early, late) = (rms(&l[30..90]), rms(&l[18000..24000]));
        assert!(late < 0.4 * early && late > 0.15 * early, "{early} {late}");
    }

    #[test]
    fn zones_by_key_velocity_layers_and_round_robin() {
        let sr = 48000.0;
        // four samples you can tell apart by their level
        let dc = |x: f32| vec![x; 24000];
        let z = |pad, root, lo, hi, vlo, vhi| Zone { pad, root, lo, hi, vlo, vhi };
        let zones = [
            z(0, 48, 0, 59, 0, 127),   // low keys
            z(1, 72, 60, 127, 0, 79),  // high keys, soft
            z(2, 72, 60, 127, 80, 127), // high keys, hard: round robin of two
            z(3, 72, 60, 127, 80, 127),
        ];
        let mut p = defaults();
        p[ATTACK] = 0.0;
        p[VELOCITY] = 0.0; // (so the level is the sample's)
        let mut s = Sampler::from_zones(sr, vec![dc(0.1), dc(0.2), dc(0.3), dc(0.4)], &zones);
        let lv = p[LEVEL] * p[LEVEL] * 1.6;
        let hit = |s: &mut Sampler, key, vel| {
            s.note(NoteEv::AllOff);
            s.note(NoteEv::On(key, vel));
            let (l, _) = run(s, &p, 512);
            ((l[400] / lv) * 10.0).round() as i32
        };
        assert_eq!(hit(&mut s, 40, 100), 1, "a low key: the low zone");
        assert_eq!(hit(&mut s, 70, 50), 2, "a high key, soft");
        let a = hit(&mut s, 70, 110);
        let b = hit(&mut s, 70, 110);
        let c = hit(&mut s, 70, 110);
        assert!([a, b] == [3, 4] || [a, b] == [4, 3], "hard: they take turns ({a} {b})");
        assert_eq!(a, c, "and round again");
        // a zone plays at its own root: an octave above it is twice as fast
        let mut s = Sampler::from_zones(sr, vec![ramp(4800)], &[z(0, 50, 0, 127, 0, 127)]);
        let mut p = defaults();
        p[ONESHOT] = 1.0;
        s.note(NoteEv::On(62, 127));
        let (l, _) = run(&mut s, &p, 6000);
        let last = l.iter().rposition(|x| x.abs() > 1e-6).unwrap();
        assert!((2380..2410).contains(&last), "an octave above its root ({last})");
        // no zone for the key: silence
        let mut s = Sampler::from_zones(sr, vec![dc(0.5)], &[z(0, 60, 60, 72, 0, 127)]);
        s.note(NoteEv::On(30, 100));
        let (l, _) = run(&mut s, &defaults(), 512);
        assert!(l.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn velocity_pan_and_mono_glide() {
        let sr = 48000.0;
        let dc = vec![0.5f32; 48000];
        let mut p = defaults();
        p[ATTACK] = 0.0;
        let level = |vel: u8, p: &[f32; N_PARAMS]| {
            let mut s = Sampler::from_samples(sr, dc.clone(), None);
            s.note(NoteEv::On(60, vel));
            run(&mut s, p, 2000).0[1500]
        };
        let (hi, lo) = (level(127, &p), level(64, &p));
        assert!((lo / hi - (64.0f32 / 127.0).powi(2)).abs() < 0.01, "as the old sampler: velocity squared");
        p[VELOCITY] = 0.0;
        assert!((level(64, &p) - hi).abs() < 1e-4, "velocity off: all the same");
        let mut p = defaults();
        p[PAN] = 0.0;
        let mut s = Sampler::from_samples(sr, dc.clone(), None);
        s.note(NoteEv::On(60, 127));
        let (l, r) = run(&mut s, &p, 2000);
        assert!(l[1500] > 0.5 && r[1500].abs() < 1e-4, "hard left");
        // mono: a second note takes over the first (one voice), gliding up
        let mut p = defaults();
        p[MONO] = 1.0;
        p[GLIDE] = 0.3;
        let mut s = Sampler::from_samples(sr, dc, None);
        s.note(NoteEv::On(60, 127));
        run(&mut s, &p, 1000);
        s.note(NoteEv::On(72, 127));
        run(&mut s, &p, 256);
        assert_eq!(s.voices.len(), 1);
        let v = s.voices[0];
        assert!(v.pitch > 60.0 && v.pitch < 72.0 && v.amp > 0.9, "gliding, no new attack: {} {}", v.pitch, v.amp);
    }
}
