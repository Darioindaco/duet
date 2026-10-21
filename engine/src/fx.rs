//! Effects and FX racks. Every mixer strip has a rack: an ordered list of
//! slots, each holding one effect. Parameter names and 0..1 knob ranges are the
//! same as the original pedalboard, so saved settings and the UI carry over.
//!
//! Realtime rule: effect objects are built (allocated) on the control thread
//! and handed to the audio thread in a Box; removed ones are handed back to be
//! freed there. The audio thread only moves Boxes around inside preallocated Vecs.

use std::f32::consts::PI;

pub const N_KINDS: usize = 6;
pub const MAX_PARAMS: usize = 3;
pub const MAX_SLOTS: usize = 8;

pub struct FxInfo {
    pub name: &'static str,
    pub params: &'static [&'static str],
    pub defaults: [f32; MAX_PARAMS],
}

pub const FX: [FxInfo; N_KINDS] = [
    FxInfo { name: "comp", params: &["threshold", "ratio", "makeup"], defaults: [0.5, 0.5, 0.3] },
    FxInfo { name: "drive", params: &["drive", "tone", "level"], defaults: [0.4, 0.5, 0.5] },
    FxInfo { name: "chorus", params: &["rate", "depth", "mix"], defaults: [0.3, 0.5, 0.5] },
    FxInfo { name: "tremolo", params: &["rate", "depth"], defaults: [0.4, 0.6, 0.0] },
    FxInfo { name: "delay", params: &["time", "feedback", "mix"], defaults: [0.35, 0.4, 0.35] },
    FxInfo { name: "reverb", params: &["size", "damp", "mix"], defaults: [0.6, 0.4, 0.3] },
];

pub fn kind_index(name: &str) -> Option<usize> {
    FX.iter().position(|f| f.name == name)
}

pub fn param_index(kind: usize, key: &str) -> Option<usize> {
    FX[kind].params.iter().position(|p| *p == key)
}

/// Rack presets: the effects to load, in order, with their settings.
pub const PRESETS: &[(&str, &[(&str, &[(&str, f32)])])] = &[
    ("Clean", &[]),
    ("Dreamy", &[
        ("chorus", &[("rate", 0.2), ("depth", 0.7), ("mix", 0.5)]),
        ("delay", &[("time", 0.4), ("feedback", 0.45), ("mix", 0.3)]),
        ("reverb", &[("size", 0.85), ("damp", 0.3), ("mix", 0.45)]),
    ]),
    ("Crunch", &[
        ("comp", &[("threshold", 0.4), ("ratio", 0.4), ("makeup", 0.2)]),
        ("drive", &[("drive", 0.6), ("tone", 0.55), ("level", 0.45)]),
        ("reverb", &[("size", 0.4), ("damp", 0.5), ("mix", 0.15)]),
    ]),
    ("Vocal", &[
        ("comp", &[("threshold", 0.35), ("ratio", 0.5), ("makeup", 0.35)]),
        ("reverb", &[("size", 0.55), ("damp", 0.45), ("mix", 0.25)]),
    ]),
    ("Surf", &[
        ("tremolo", &[("rate", 0.45), ("depth", 0.5)]),
        ("reverb", &[("size", 0.75), ("damp", 0.2), ("mix", 0.5)]),
    ]),
];

/// What a slot holds, without the DSP state: the control side's mirror.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotCfg {
    pub kind: usize,
    pub on: bool,
    pub p: [f32; MAX_PARAMS],
}

impl SlotCfg {
    pub fn new(kind: usize) -> Self {
        SlotCfg { kind, on: true, p: FX[kind].defaults }
    }
}

pub fn preset_slots(name: &str) -> Option<Vec<SlotCfg>> {
    let (_, cfg) = PRESETS.iter().find(|(n, _)| *n == name)?;
    Some(
        cfg.iter()
            .map(|(fx, params)| {
                let mut s = SlotCfg::new(kind_index(fx).unwrap());
                for (k, v) in params.iter() {
                    s.p[param_index(s.kind, k).unwrap()] = *v;
                }
                s
            })
            .collect(),
    )
}

pub trait Effect: Send {
    fn process(&mut self, p: &[f32; MAX_PARAMS], buf: &mut [f32]);
    fn reset(&mut self) {}
}

pub fn make(kind: usize, sr: f32) -> Box<dyn Effect> {
    match kind {
        0 => Box::new(Compressor::new(sr)),
        1 => Box::new(Drive::new(sr)),
        2 => Box::new(Chorus::new(sr)),
        3 => Box::new(Tremolo { phase: 0.0, sr }),
        4 => Box::new(Delay::new(sr)),
        _ => Box::new(Reverb::new(sr)),
    }
}

/// A slot as the audio thread holds it: settings + the effect's DSP state.
pub struct Slot {
    pub cfg: SlotCfg,
    pub fx: Box<dyn Effect>,
}

impl Slot {
    pub fn new(cfg: SlotCfg, sr: f32) -> Box<Slot> {
        Box::new(Slot { cfg, fx: make(cfg.kind, sr) })
    }
}

pub struct Rack {
    pub slots: Vec<Box<Slot>>,
}

impl Rack {
    pub fn new() -> Self {
        Rack { slots: Vec::with_capacity(MAX_SLOTS) }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for s in self.slots.iter_mut() {
            if s.cfg.on {
                s.fx.process(&s.cfg.p, buf);
            }
        }
    }

    /// No allocation: capacity is reserved up front and inserts are capped.
    pub fn insert(&mut self, pos: usize, slot: Box<Slot>) -> Result<(), Box<Slot>> {
        if self.slots.len() >= MAX_SLOTS {
            return Err(slot);
        }
        let pos = pos.min(self.slots.len());
        self.slots.insert(pos, slot);
        Ok(())
    }

    pub fn remove(&mut self, pos: usize) -> Option<Box<Slot>> {
        (pos < self.slots.len()).then(|| self.slots.remove(pos))
    }

    pub fn move_slot(&mut self, from: usize, to: usize) {
        if from < self.slots.len() {
            let s = self.slots.remove(from);
            let to = to.min(self.slots.len());
            self.slots.insert(to, s);
        }
    }

    pub fn set_on(&mut self, pos: usize, on: bool) {
        if let Some(s) = self.slots.get_mut(pos) {
            if on && !s.cfg.on {
                s.fx.reset(); // don't replay a stale tail from last time
            }
            s.cfg.on = on;
        }
    }
}

// --- building blocks ---------------------------------------------------------------

struct Ring {
    buf: Vec<f32>,
    pos: usize,
}

impl Ring {
    fn new(n: usize) -> Self {
        Ring { buf: vec![0.0; n.max(2)], pos: 0 }
    }
    #[inline]
    fn write(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % self.buf.len();
    }
    /// Sample written `d` samples before the next write (d >= 1).
    #[inline]
    fn tap(&self, d: usize) -> f32 {
        let n = self.buf.len();
        self.buf[(self.pos + n - (d % n)) % n]
    }
    #[inline]
    fn tap_frac(&self, d: f32) -> f32 {
        let i = d.floor();
        let frac = d - i;
        let a = self.tap(i as usize);
        let b = self.tap(i as usize + 1);
        a + (b - a) * frac
    }
    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

#[inline]
fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

// --- effects -------------------------------------------------------------------

struct Compressor {
    env: f32,
    att: f32,
    rel: f32,
}

impl Compressor {
    fn new(sr: f32) -> Self {
        Compressor { env: 0.0, att: 1.0 - (-1.0 / (0.005 * sr)).exp(), rel: 1.0 - (-1.0 / (0.15 * sr)).exp() }
    }
}

impl Effect for Compressor {
    fn process(&mut self, p: &[f32; 3], buf: &mut [f32]) {
        let thr_db = -40.0 + 36.0 * p[0]; // -40 .. -4 dBFS
        let ratio = 1.0 + 11.0 * p[1]; // 1:1 .. 12:1
        let makeup = 18.0 * p[2];
        for x in buf.iter_mut() {
            let a = x.abs();
            let k = if a > self.env { self.att } else { self.rel };
            self.env += (a - self.env) * k;
            let env_db = 20.0 * (self.env + 1e-9).log10();
            let over = (env_db - thr_db).max(0.0);
            *x *= db_to_lin(-(over - over / ratio) + makeup);
        }
    }
}

struct Drive {
    lp: f32,
    coef: f32,
}

impl Drive {
    fn new(sr: f32) -> Self {
        Drive { lp: 0.0, coef: 1.0 - (-2.0 * PI * 1800.0 / sr).exp() }
    }
}

impl Effect for Drive {
    fn process(&mut self, p: &[f32; 3], buf: &mut [f32]) {
        let g = 1.0 + 40.0 * p[0] * p[0];
        let norm = 1.0 / g.tanh().sqrt();
        let (tone, level) = (p[1], 0.2 + 1.3 * p[2]);
        for x in buf.iter_mut() {
            let y = (*x * g).tanh() * norm;
            self.lp += (y - self.lp) * self.coef;
            *x = (self.lp * (1.0 - tone) + y * tone) * level;
        }
    }
}

struct Chorus {
    ring: Ring,
    phase: f32,
    sr: f32,
}

impl Chorus {
    fn new(sr: f32) -> Self {
        Chorus { ring: Ring::new((sr * 0.05) as usize), phase: 0.0, sr }
    }
}

impl Effect for Chorus {
    fn process(&mut self, p: &[f32; 3], buf: &mut [f32]) {
        let rate = 0.1 + 3.0 * p[0];
        let inc = 2.0 * PI * rate / self.sr;
        let base = 0.012 * self.sr;
        let depth = 0.006 * self.sr * p[1];
        let m = p[2];
        for x in buf.iter_mut() {
            self.ring.write(*x);
            let d1 = base + depth * self.phase.sin();
            let d2 = base + depth * (self.phase + PI / 2.0).sin();
            let wet = 0.5 * (self.ring.tap_frac(d1) + self.ring.tap_frac(d2));
            *x = *x * (1.0 - 0.5 * m) + wet * m;
            self.phase = (self.phase + inc) % (2.0 * PI);
        }
    }
    fn reset(&mut self) {
        self.ring.clear();
    }
}

struct Tremolo {
    phase: f32,
    sr: f32,
}

impl Effect for Tremolo {
    fn process(&mut self, p: &[f32; 3], buf: &mut [f32]) {
        let rate = 0.5 + 11.5 * p[0];
        let inc = 2.0 * PI * rate / self.sr;
        for x in buf.iter_mut() {
            *x *= 1.0 - p[1] * (0.5 + 0.5 * self.phase.sin());
            self.phase = (self.phase + inc) % (2.0 * PI);
        }
    }
}

struct Delay {
    ring: Ring,
    lp: f32,
    sr: f32,
}

impl Delay {
    fn new(sr: f32) -> Self {
        Delay { ring: Ring::new((sr * 1.6) as usize), lp: 0.0, sr }
    }
}

impl Effect for Delay {
    fn process(&mut self, p: &[f32; 3], buf: &mut [f32]) {
        let d = ((0.06 + 1.1 * p[0]) * self.sr) as usize;
        let fb = 0.92 * p[1];
        for x in buf.iter_mut() {
            let echoed = self.ring.tap(d);
            self.lp += (echoed - self.lp) * 0.5; // repeats get darker, like tape
            self.ring.write(*x + self.lp * fb);
            *x += self.lp * p[2];
        }
    }
    fn reset(&mut self) {
        self.ring.clear();
        self.lp = 0.0;
    }
}

struct Reverb {
    combs: Vec<(Ring, usize, f32)>, // ring, delay, filter store
    aps: Vec<(Ring, usize)>,
}

impl Reverb {
    const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const ALLPASS: [usize; 4] = [556, 441, 341, 225];

    fn new(sr: f32) -> Self {
        let scale = sr / 44100.0;
        Reverb {
            combs: Self::COMBS.iter().map(|&c| {
                let d = (c as f32 * scale) as usize;
                (Ring::new(d + 1), d, 0.0)
            }).collect(),
            aps: Self::ALLPASS.iter().map(|&a| {
                let d = (a as f32 * scale) as usize;
                (Ring::new(d + 1), d)
            }).collect(),
        }
    }
}

impl Effect for Reverb {
    fn process(&mut self, p: &[f32; 3], buf: &mut [f32]) {
        let fb = 0.7 + 0.28 * p[0];
        let damp = p[1];
        let m = p[2];
        for x in buf.iter_mut() {
            let inp = *x * 0.015;
            let mut out = 0.0;
            for (ring, d, store) in self.combs.iter_mut() {
                let y = ring.tap(*d);
                *store = y * (1.0 - damp) + *store * damp;
                ring.write(inp + *store * fb);
                out += y;
            }
            for (ring, d) in self.aps.iter_mut() {
                let b = ring.tap(*d);
                ring.write(out + b * 0.5);
                out = b - out;
            }
            *x = *x * (1.0 - 0.5 * m) + out * 3.0 * m;
        }
    }
    fn reset(&mut self) {
        for (r, _, s) in self.combs.iter_mut() {
            r.clear();
            *s = 0.0;
        }
        for (r, _) in self.aps.iter_mut() {
            r.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize) -> Vec<f32> {
        let mut s = 12345u32;
        (0..n).map(|_| {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            ((s >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.6
        }).collect()
    }

    fn rack_of(cfgs: &[SlotCfg]) -> Rack {
        let mut r = Rack::new();
        for (i, c) in cfgs.iter().enumerate() {
            r.insert(i, Slot::new(*c, 48000.0)).ok().unwrap();
        }
        r
    }

    #[test]
    fn every_effect_is_finite_bounded_and_audible() {
        for kind in 0..N_KINDS {
            for block in [64usize, 128, 512] {
                let mut r = rack_of(&[SlotCfg::new(kind)]);
                let mut x = noise(96000);
                for c in x.chunks_mut(block) {
                    r.process(c);
                }
                assert!(x.iter().all(|v| v.is_finite()), "{}", FX[kind].name);
                let peak = x.iter().fold(0f32, |a, v| a.max(v.abs()));
                assert!(peak < 4.0, "{} peak {}", FX[kind].name, peak);
                let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
                assert!(rms > 0.01, "{}", FX[kind].name);
            }
        }
    }

    #[test]
    fn delay_echo_lands_on_time() {
        let mut s = SlotCfg::new(4);
        s.p = [0.0, 0.0, 0.35]; // 60 ms, no repeats
        let mut r = rack_of(&[s]);
        let mut x = vec![0f32; 48000];
        x[0] = 1.0;
        for c in x.chunks_mut(128) {
            r.process(c);
        }
        let echo = (100..x.len()).max_by(|&a, &b| x[a].abs().total_cmp(&x[b].abs())).unwrap();
        assert!((echo as i64 - 2880).abs() <= 1, "echo at {}", echo);
    }

    #[test]
    fn reverb_decays() {
        let mut s = SlotCfg::new(5);
        s.p = [1.0, 0.0, 0.3];
        let mut r = rack_of(&[s]);
        let mut x = vec![0f32; 48000 * 8];
        x[..128].fill(1.0);
        for c in x.chunks_mut(128) {
            r.process(c);
        }
        let peak = |s: &[f32]| s.iter().fold(0f32, |a, v| a.max(v.abs()));
        assert!(peak(&x[x.len() - 48000..]) < peak(&x[..48000]) * 0.1);
    }

    #[test]
    fn rack_order_matters_and_never_grows_past_capacity() {
        // drive then reverb != reverb then drive
        let (d, v) = (SlotCfg::new(1), SlotCfg::new(5));
        let mut a = rack_of(&[d, v]);
        let mut b = rack_of(&[v, d]);
        let (mut xa, mut xb) = (noise(4800), noise(4800));
        a.process(&mut xa);
        b.process(&mut xb);
        assert!(xa.iter().zip(&xb).any(|(p, q)| (p - q).abs() > 1e-3));
        // move puts it back in the same order
        b.move_slot(1, 0);
        assert_eq!(b.slots[0].cfg.kind, 1);
        let mut full = Rack::new();
        let cap = full.slots.capacity();
        for i in 0..MAX_SLOTS {
            assert!(full.insert(i, Slot::new(SlotCfg::new(0), 48000.0)).is_ok());
        }
        assert!(full.insert(0, Slot::new(SlotCfg::new(0), 48000.0)).is_err());
        assert_eq!(full.slots.capacity(), cap, "rack reallocated");
    }

    #[test]
    fn presets_match_original_names() {
        for name in ["Clean", "Dreamy", "Crunch", "Vocal", "Surf"] {
            assert!(preset_slots(name).is_some());
        }
        let surf: Vec<&str> = preset_slots("Surf").unwrap().iter().map(|s| FX[s.kind].name).collect();
        assert_eq!(surf, ["tremolo", "reverb"]);
    }
}
