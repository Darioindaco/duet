//! More of Duet's own plugins: De-esser, Transient shaper, Imager, Gate, Filter, Lo-fi, Utility,
//! Doubler, and the hall/room/plate reverb the Reverb uses for those types. Same rules as plugins.rs.

use crate::fx::{Effect, Key, Params};
use crate::kit::*;

/// A delay line with fractional reads (allocated once).
pub struct Line {
    buf: Vec<f32>,
    pos: usize,
}

impl Line {
    pub fn new(n: usize) -> Self {
        Line { buf: vec![0.0; n.max(4)], pos: 0 }
    }
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % self.buf.len();
    }
    /// what was pushed `d` samples before the next push (1 = the newest), with a fraction
    #[inline]
    pub fn read(&self, d: f32) -> f32 {
        let n = self.buf.len();
        let d = d.clamp(1.0, (n - 2) as f32);
        let i = d.floor();
        let fr = d - i;
        let a = self.buf[(self.pos + n - i as usize) % n];
        let b = self.buf[(self.pos + n - i as usize - 1) % n];
        a + (b - a) * fr
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

/// A cheap noise source.
#[derive(Clone, Copy)]
pub struct Noise(u32);

impl Noise {
    pub fn new(seed: u32) -> Self {
        Noise(seed | 1)
    }
    #[inline]
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

// --- De-esser ---------------------------------------------------------------------------------

/// freq 2..12 kHz, threshold -60..0 dB, range 0..24 dB, mode (split: only the highs / wide: all),
/// listen (hear what it listens to)
pub struct DeEsser {
    sr: f32,
    det: [Biquad; 2],
    xo: [Crossover; 2],
    env: Env,
    f_at: f32,
    gr: f32,
    scope: [f32; 1],
}

impl DeEsser {
    pub fn new(sr: f32) -> Self {
        DeEsser { sr, det: [Biquad::new(); 2], xo: [Crossover::default(); 2], env: Env::default(), f_at: 0.0, gr: 0.0, scope: [-120.0] }
    }
}

impl Effect for DeEsser {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let f = logk(p[0], 2000.0, 12000.0);
        let thr = -60.0 + 60.0 * p[1];
        let range = 24.0 * p[2];
        let wide = choice(p[3], 2) == 1;
        let listen = p[4] >= 0.5;
        if (f - self.f_at).abs() > 0.5 {
            self.f_at = f;
            let c = rbj::highpass(f as f64, 0.7071, self.sr as f64);
            self.det[0].c = c;
            self.det[1].c = c;
            for x in &mut self.xo {
                x.set(f, self.sr);
            }
        }
        let (att, rel) = (coef(0.5, self.sr), coef(60.0, self.sr));
        let mut most = 0f32;
        for k in 0..l.len().min(r.len()) {
            let a = self.det[0].run(l[k]);
            let b = self.det[1].run(r[k]);
            let e = self.env.run(a.abs().max(b.abs()), att, rel);
            let lvl = db(e);
            let red = ((lvl - thr) * 0.8).clamp(0.0, range);
            most = most.max(red);
            let g = lin(-red);
            if listen {
                l[k] = a;
                r[k] = b;
            } else if wide {
                l[k] *= g;
                r[k] *= g;
            } else {
                let (lo_l, hi_l) = self.xo[0].split(l[k]);
                let (lo_r, hi_r) = self.xo[1].split(r[k]);
                l[k] = lo_l + hi_l * g;
                r[k] = lo_r + hi_r * g;
            }
            self.scope[0] = lvl;
        }
        self.gr = most;
    }
    fn gain_reduction(&self) -> f32 {
        self.gr
    }
    fn scope(&self) -> &[f32] {
        &self.scope
    }
}

// --- Transient shaper ----------------------------------------------------------------------------

/// attack -100..+100%, sustain -100..+100%, speed (how short a hit counts), output -12..+12 dB, mix
pub struct Transient {
    sr: f32,
    fast: Env,
    slow: Env,
    tail_fast: Env,
    tail_slow: Env,
    g: f32,
    scope: [f32; 2],
}

impl Transient {
    pub fn new(sr: f32) -> Self {
        Transient { sr, fast: Env::default(), slow: Env::default(), tail_fast: Env::default(), tail_slow: Env::default(), g: 0.0, scope: [0.0; 2] }
    }
}

impl Effect for Transient {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let (att_amt, sus_amt) = (p[0] * 2.0 - 1.0, p[1] * 2.0 - 1.0);
        let speed = 0.5 + 1.5 * p[2]; // x0.5 .. x2 of the times
        let (gout, mix) = (lin(-12.0 + 24.0 * p[3]), p[4]);
        let sr = self.sr;
        let (a1, a2, r1) = (coef(0.3 * speed, sr), coef(20.0 * speed, sr), coef(60.0 * speed, sr));
        let (r_fast, r_slow) = (coef(40.0 * speed, sr), coef(400.0 * speed, sr));
        let sm = coef(1.0, sr);
        let (mut ta, mut ts) = (0f32, 0f32);
        for k in 0..l.len().min(r.len()) {
            let x = l[k].abs().max(r[k].abs()) + 1e-9;
            // a hit: the fast follower ahead of the slow one; a tail: the slow-to-let-go one above the quick one
            let hit = (db(self.fast.run(x, a1, r1)) - db(self.slow.run(x, a2, r1))).max(0.0);
            let tail = (db(self.tail_slow.run(x, a2, r_slow)) - db(self.tail_fast.run(x, a2, r_fast))).max(0.0);
            let want = (att_amt * hit * 1.5 + sus_amt * tail * 1.2).clamp(-24.0, 18.0);
            self.g += (want - self.g) * sm;
            let g = lin(self.g);
            l[k] = (l[k] + (l[k] * g - l[k]) * mix) * gout;
            r[k] = (r[k] + (r[k] * g - r[k]) * mix) * gout;
            ta = ta.max(att_amt * hit);
            ts = ts.max(sus_amt * tail);
        }
        self.scope = [self.g, ta.abs().max(ts.abs())];
    }
    fn scope(&self) -> &[f32] {
        &self.scope
    }
}

// --- Imager -------------------------------------------------------------------------------------

/// width 0..200%, low width, high width (0..200%), xover 100..5000 Hz, mono bass below 20..500 Hz
/// (0 = off), balance, output -12..+12 dB
pub struct Imager {
    sr: f32,
    xo: [Crossover; 2],
    mono: [Crossover; 2],
    corr: (f64, f64, f64),
    pts: [f32; 66],
    tick: usize,
    pi: usize,
}

impl Imager {
    pub fn new(sr: f32) -> Self {
        Imager { sr, xo: [Crossover::default(); 2], mono: [Crossover::default(); 2], corr: (0.0, 0.0, 0.0), pts: [0.0; 66], tick: 0, pi: 0 }
    }
}

impl Effect for Imager {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let width = 2.0 * p[0];
        let (wl, wh) = (2.0 * p[1], 2.0 * p[2]);
        let xf = logk(p[3], 100.0, 5000.0);
        let mono_on = p[4] > 0.005;
        let mf = logk(p[4], 20.0, 500.0);
        let bal = p[5] * 2.0 - 1.0;
        let gout = lin(-12.0 + 24.0 * p[6]);
        for x in &mut self.xo {
            x.set(xf, self.sr);
        }
        for x in &mut self.mono {
            x.set(mf, self.sr);
        }
        let (gl, gr) = ((1.0 - bal).min(1.0), (1.0 + bal).min(1.0));
        let n = l.len().min(r.len());
        let decay = 0.9999f64.powi(n as i32 * 2);
        let (mut sxy, mut sxx, mut syy) = (0f64, 0f64, 0f64);
        for k in 0..n {
            let (lo_l, hi_l) = self.xo[0].split(l[k]);
            let (lo_r, hi_r) = self.xo[1].split(r[k]);
            let w = |a: f32, b: f32, wd: f32| {
                let (m, s) = (0.5 * (a + b), 0.5 * (a - b) * wd * width);
                (m + s, m - s)
            };
            let (al, ar) = w(lo_l, lo_r, wl);
            let (bl, br) = w(hi_l, hi_r, wh);
            let (mut a, mut b) = (al + bl, ar + br);
            if mono_on {
                // below the mono frequency both sides get the same
                let (la, ha) = self.mono[0].split(a);
                let (lb, hb) = self.mono[1].split(b);
                let m = 0.5 * (la + lb);
                a = m + ha;
                b = m + hb;
            }
            a *= gl * gout;
            b *= gr * gout;
            l[k] = a;
            r[k] = b;
            sxy += (a * b) as f64;
            sxx += (a * a) as f64;
            syy += (b * b) as f64;
            self.tick += 1;
            if self.tick % 64 == 0 {
                self.pts[2 + 2 * self.pi] = a;
                self.pts[3 + 2 * self.pi] = b;
                self.pi = (self.pi + 1) % 32;
            }
        }
        self.corr = (self.corr.0 * decay + sxy, self.corr.1 * decay + sxx, self.corr.2 * decay + syy);
        let den = (self.corr.1 * self.corr.2).sqrt();
        self.pts[0] = if den > 1e-9 { (self.corr.0 / den) as f32 } else { 1.0 };
        let (el, er) = (self.corr.1.sqrt(), self.corr.2.sqrt());
        self.pts[1] = if el + er > 1e-9 { ((er - el) / (el + er)) as f32 } else { 0.0 };
    }
    fn scope(&self) -> &[f32] {
        &self.pts
    }
}

// --- Gate ----------------------------------------------------------------------------------------

/// threshold -80..0 dB, range 0..80 dB, attack 0.05..50 ms, hold 0..500 ms, release 5..2000 ms,
/// hysteresis 0..12 dB, the detector's low cut 20..2000 Hz (0 off) and high cut 500..20000 Hz (1 off),
/// listen
pub struct Gate {
    sr: f32,
    env: Env,
    open: bool,
    held: f32,
    g: f32,
    hp: [Biquad; 2],
    lp: [Biquad; 2],
    f_at: (f32, f32),
    gr: f32,
    scope: [f32; 2],
}

impl Gate {
    pub fn new(sr: f32) -> Self {
        Gate { sr, env: Env::default(), open: false, held: 0.0, g: 0.0, hp: [Biquad::new(); 2], lp: [Biquad::new(); 2], f_at: (-1.0, -1.0), gr: 0.0, scope: [-120.0, 0.0] }
    }
}

impl Effect for Gate {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        self.process_keyed(p, l, r, None);
    }
    fn process_keyed(&mut self, p: &Params, l: &mut [f32], r: &mut [f32], key: Key) {
        let thr = -80.0 + 80.0 * p[0];
        let floor = lin(-80.0 * p[1]);
        let att = coef(logk(p[2], 0.05, 50.0), self.sr);
        let hold = 0.5 * p[3] * self.sr;
        let rel = coef(logk(p[4], 5.0, 2000.0), self.sr);
        let hyst = 12.0 * p[5];
        let (hp_on, lp_on) = (p[6] > 0.005, p[7] < 0.995);
        let (hf, lf) = (logk(p[6], 20.0, 2000.0), logk(p[7], 500.0, 20000.0));
        let listen = p[8] >= 0.5;
        if (hf, lf) != self.f_at {
            self.f_at = (hf, lf);
            let (h, lo) = (rbj::highpass(hf as f64, 0.7071, self.sr as f64), rbj::lowpass(lf as f64, 0.7071, self.sr as f64));
            for b in &mut self.hp {
                b.c = h;
            }
            for b in &mut self.lp {
                b.c = lo;
            }
        }
        let det_rel = coef(20.0, self.sr);
        let mut most = 0f32;
        for k in 0..l.len().min(r.len()) {
            let (mut a, mut b) = match key {
                Some((kl, kr)) if k < kl.len() => (kl[k], kr[k]),
                Some(_) => (0.0, 0.0),
                None => (l[k], r[k]),
            };
            if hp_on {
                a = self.hp[0].run(a);
                b = self.hp[1].run(b);
            }
            if lp_on {
                a = self.lp[0].run(a);
                b = self.lp[1].run(b);
            }
            let lvl = db(self.env.run(a.abs().max(b.abs()), 1.0, det_rel));
            if lvl > thr {
                self.open = true;
                self.held = hold;
            } else if lvl < thr - hyst {
                if self.held > 0.0 {
                    self.held -= 1.0;
                } else {
                    self.open = false;
                }
            }
            let want = if self.open { 1.0 } else { 0.0 };
            self.g += (want - self.g) * if want > self.g { att } else { rel };
            let g = floor + (1.0 - floor) * self.g;
            most = most.max(-db(g));
            if listen {
                l[k] = a;
                r[k] = b;
            } else {
                l[k] *= g;
                r[k] *= g;
            }
            self.scope = [lvl, self.g];
        }
        self.gr = most;
    }
    fn gain_reduction(&self) -> f32 {
        self.gr
    }
    fn scope(&self) -> &[f32] {
        &self.scope
    }
}

// --- Filter --------------------------------------------------------------------------------------

/// type (low/high/band pass, notch), slope 12/24, cutoff 20..20k, resonance, drive 0..24 dB, LFO rate
/// (in time: 1/16 .. 4 bars), LFO shape, LFO depth -4..+4 octaves, envelope -4..+4 octaves, mix
pub struct Filter {
    sr: f32,
    svf: [[Svf; 2]; 2],
    env: Env,
    rnd: Noise,
    hold: f32,
    last_ph: f64,
    told: f64,
    own: f64,
    scope: [f32; 1],
}

pub const FILTER_RATES: [f64; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

impl Filter {
    pub fn new(sr: f32) -> Self {
        Filter { sr, svf: [[Svf::default(); 2]; 2], env: Env::default(), rnd: Noise::new(77), hold: 0.0, last_ph: 0.0, told: f64::NAN, own: 0.0, scope: [1000.0] }
    }
}

impl Effect for Filter {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let kind = choice(p[0], 4);
        let stages = if choice(p[1], 2) == 1 { 2 } else { 1 };
        let base = logk(p[2], 20.0, 20000.0);
        let k = 2.0 - 1.95 * p[3].clamp(0.0, 1.0);
        let drive = lin(24.0 * p[4]);
        let period = FILTER_RATES[choice(p[5], FILTER_RATES.len())];
        let shape = choice(p[6], 5);
        let lfo_oct = (p[7] * 2.0 - 1.0) * 4.0;
        let env_oct = (p[8] * 2.0 - 1.0) * 4.0;
        let mix = p[9];
        let t = song_time();
        let bps = t.bpm / 60.0 / self.sr as f64;
        let start = if t.beat != self.told { t.beat } else { self.own };
        self.told = t.beat;
        let n = l.len().min(r.len());
        self.own = start + n as f64 * bps;
        let (ea, er) = (coef(5.0, self.sr), coef(150.0, self.sr));
        let mut f_now = base;
        for i in 0..n {
            let ph = ((start + i as f64 * bps) / period).rem_euclid(1.0);
            if ph < self.last_ph {
                self.hold = self.rnd.next();
            }
            self.last_ph = ph;
            let ph = ph as f32;
            let lfo = match shape {
                0 => (2.0 * std::f32::consts::PI * ph).sin(),
                1 => 1.0 - 4.0 * (ph - 0.5).abs(),
                2 => 2.0 * ph - 1.0,
                3 => if ph < 0.5 { 1.0 } else { -1.0 },
                _ => self.hold,
            };
            let e = self.env.run(l[i].abs().max(r[i].abs()), ea, er).min(1.0);
            let oct = lfo * lfo_oct + e * env_oct * 2.0;
            f_now = (base * 2f32.powf(oct)).clamp(20.0, self.sr * 0.45);
            for (ch, x) in [&mut l[i], &mut r[i]].into_iter().enumerate() {
                let dry = *x;
                let mut y = if drive > 1.001 { (*x * drive).tanh() / drive.sqrt() } else { *x };
                for s in 0..stages {
                    let (lp, bp, hp) = self.svf[s][ch].run(y, f_now, if s + 1 == stages { k } else { 1.414 }, self.sr);
                    y = match kind {
                        0 => lp,
                        1 => hp,
                        2 => bp,
                        _ => lp + hp,
                    };
                }
                *x = dry + (y - dry) * mix;
            }
        }
        self.scope = [f_now];
    }
    fn reset(&mut self) {
        self.svf = [[Svf::default(); 2]; 2];
    }
    fn scope(&self) -> &[f32] {
        &self.scope
    }
}

// --- Lo-fi ---------------------------------------------------------------------------------------

/// bits 2..16, rate 500..48000 Hz, wow, flutter, noise (hiss and crackle), tone (high cut 1..20 kHz),
/// mix, output -12..+12 dB
pub struct LoFi {
    sr: f32,
    line: [Line; 2],
    held: [f32; 2],
    acc: f32,
    wow_ph: f32,
    flut_ph: f32,
    drift: f32,
    rnd: Noise,
    tone: [Biquad; 2],
    hiss: [Biquad; 2],
    tone_at: f32,
}

impl LoFi {
    pub fn new(sr: f32) -> Self {
        let n = (sr * 0.05) as usize;
        let mut hiss = [Biquad::new(); 2];
        for h in &mut hiss {
            h.c = rbj::bandpass(5000.0, 0.5, sr as f64);
        }
        LoFi { sr, line: [Line::new(n), Line::new(n)], held: [0.0; 2], acc: 0.0, wow_ph: 0.0, flut_ph: 0.0, drift: 0.0,
            rnd: Noise::new(1234), tone: [Biquad::new(); 2], hiss, tone_at: 0.0 }
    }
}

impl Effect for LoFi {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let bits = 2.0 + 14.0 * p[0];
        let rate = logk(p[1], 500.0, 48000.0).min(self.sr);
        let (wow, flutter, noise) = (p[2], p[3], p[4]);
        let tone = logk(p[5], 1000.0, 20000.0);
        let (mix, gout) = (p[6], lin(-12.0 + 24.0 * p[7]));
        if (tone - self.tone_at).abs() > 1.0 {
            self.tone_at = tone;
            for b in &mut self.tone {
                b.c = rbj::lowpass(tone as f64, 0.7071, self.sr as f64);
            }
        }
        let q = 2f32.powf(bits - 1.0);
        let step = rate / self.sr;
        let base = self.sr * 0.012;
        for k in 0..l.len().min(r.len()) {
            // wow (slow, drifting) and flutter (fast) pull the pitch around through a moving delay
            self.wow_ph = (self.wow_ph + 0.55 / self.sr) % 1.0;
            self.flut_ph = (self.flut_ph + 7.3 / self.sr) % 1.0;
            self.drift += (self.rnd.next() * 0.5 - self.drift) * 0.00005;
            let m = wow * (2.0 * std::f32::consts::PI * self.wow_ph).sin() * self.sr * 0.004 * (1.0 + self.drift)
                + flutter * (2.0 * std::f32::consts::PI * self.flut_ph).sin() * self.sr * 0.0004;
            // the sample-rate reducer: hold each sample until the slower clock ticks
            self.acc += step;
            let tick = self.acc >= 1.0;
            if tick {
                self.acc -= 1.0;
            }
            let crackle = if noise > 0.0 && self.rnd.next() > 1.0 - 0.0004 * noise { self.rnd.next() * 0.6 * noise } else { 0.0 };
            for (ch, x) in [&mut l[k], &mut r[k]].into_iter().enumerate() {
                let dry = *x;
                self.line[ch].push(*x);
                let mut y = if wow + flutter > 0.0 { self.line[ch].read(base + m) } else { *x };
                if tick || step >= 0.999 {
                    self.held[ch] = y;
                }
                y = self.held[ch];
                if bits < 15.9 {
                    y = (y * q).round() / q;
                }
                y = self.tone[ch].run(y);
                let hiss = self.hiss[ch].run(self.rnd.next()) * 0.02 * noise * noise;
                y += hiss + crackle;
                *x = (dry + (y - dry) * mix) * gout;
            }
        }
    }
    fn reset(&mut self) {
        for l in &mut self.line {
            l.clear();
        }
    }
}

// --- Utility -------------------------------------------------------------------------------------

/// gain -24..+24 dB, pan, width 0..200%, flip left, flip right, mono, swap, dc (take out DC)
pub struct Utility {
    dc: [DcBlock; 2],
}

impl Utility {
    pub fn new(_sr: f32) -> Self {
        Utility { dc: [DcBlock::default(); 2] }
    }
}

impl Effect for Utility {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let g = lin(-24.0 + 48.0 * p[0]);
        let pan = p[1] * 2.0 - 1.0;
        // equal power: the middle stays at 0 dB
        let (pl, pr) = ((1.0 - pan).min(1.0), (1.0 + pan).min(1.0));
        let width = 2.0 * p[2];
        let (fl, fr) = (if p[3] >= 0.5 { -1.0 } else { 1.0 }, if p[4] >= 0.5 { -1.0 } else { 1.0 });
        let (mono, swap, dc) = (p[5] >= 0.5, p[6] >= 0.5, p[7] >= 0.5);
        for k in 0..l.len().min(r.len()) {
            let (mut a, mut b) = (l[k] * fl, r[k] * fr);
            if swap {
                std::mem::swap(&mut a, &mut b);
            }
            let (m, s) = (0.5 * (a + b), 0.5 * (a - b) * if mono { 0.0 } else { width });
            a = m + s;
            b = m - s;
            if dc {
                a = self.dc[0].run(a);
                b = self.dc[1].run(b);
            }
            l[k] = a * g * pl;
            r[k] = b * g * pr;
        }
    }
}

// --- Doubler -------------------------------------------------------------------------------------

/// detune 0..50 cents, delay 0..40 ms, width, voices 1..4 pairs, mix, low cut on the copies
/// (20..1000 Hz, 0 = off)
pub struct Doubler {
    sr: f32,
    line: [Line; 2],
    ph: [f32; 8],
    hp: [Biquad; 2],
    hp_at: f32,
}

impl Doubler {
    pub fn new(sr: f32) -> Self {
        let n = (sr * 0.12) as usize;
        Doubler { sr, line: [Line::new(n), Line::new(n)], ph: [0.0, 0.13, 0.27, 0.41, 0.55, 0.69, 0.83, 0.97], hp: [Biquad::new(); 2], hp_at: 0.0 }
    }
}

impl Effect for Doubler {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let cents = 50.0 * p[0];
        let delay = 0.040 * p[1] * self.sr;
        let width = p[2];
        let voices = 1 + choice(p[3], 4);
        let mix = p[4];
        let hp_on = p[5] > 0.005;
        let hf = logk(p[5], 20.0, 1000.0);
        if hp_on && (hf - self.hp_at).abs() > 0.5 {
            self.hp_at = hf;
            for b in &mut self.hp {
                b.c = rbj::highpass(hf as f64, 0.7071, self.sr as f64);
            }
        }
        // each voice: a delay swept like a saw shifts the pitch a little (two taps crossfaded so
        // the jump back is never heard)
        let win = self.sr * 0.05;
        let nv = 2 * voices;
        for k in 0..l.len().min(r.len()) {
            self.line[0].push(l[k]);
            self.line[1].push(r[k]);
            let (mut wl, mut wr) = (0f32, 0f32);
            for v in 0..nv {
                let up = v % 2 == 0;
                let amt = cents * ((v / 2) as f32 + 1.0) / voices as f32 * if up { 1.0 } else { -1.0 };
                let ratio = 2f32.powf(amt / 1200.0);
                self.ph[v] = (self.ph[v] + (1.0 - ratio) / win).rem_euclid(1.0);
                let src = if up { 0 } else { 1 };
                let tap = |ph: f32, line: &Line| line.read(delay + 2.0 + ph * win);
                let a = self.ph[v];
                let b = (a + 0.5) % 1.0;
                let ga = (std::f32::consts::PI * a).sin();
                let gb = (std::f32::consts::PI * b).sin();
                let y = tap(a, &self.line[src]) * ga + tap(b, &self.line[src]) * gb;
                // spread across the stereo field, alternating sides
                let pan = if up { -width } else { width } * (0.5 + 0.5 * ((v / 2) as f32 + 1.0) / voices as f32);
                wl += y * (1.0 - pan) * 0.5;
                wr += y * (1.0 + pan) * 0.5;
            }
            let norm = 1.0 / (voices as f32).sqrt();
            wl *= norm;
            wr *= norm;
            if hp_on {
                wl = self.hp[0].run(wl);
                wr = self.hp[1].run(wr);
            }
            l[k] += wl * mix;
            r[k] += wr * mix;
        }
    }
    fn reset(&mut self) {
        for l in &mut self.line {
            l.clear();
        }
    }
}

// --- the reverb's rooms (a feedback delay network) --------------------------------------------------

const FDN: usize = 8;

/// Eight delay lines feeding each other through a mixing matrix, each a little darker every pass,
/// gently moving so the tail doesn't ring. Diffusers in front smear the input into a dense wash.
pub struct Fdn {
    sr: f32,
    lines: Vec<Line>,
    lp: [f32; FDN],
    diff: Vec<(Line, f32)>,
    pre: Line,
    ph: [f32; FDN],
    lc: [Biquad; 2],
    hc: [Biquad; 2],
    f_at: (f32, f32),
}

/// lengths in ms per type: room, hall, plate
const FDN_MS: [[f32; FDN]; 3] = [
    [11.3, 13.7, 17.9, 19.1, 23.3, 26.9, 29.3, 31.7],
    [37.1, 43.3, 51.7, 59.3, 67.9, 73.1, 83.3, 97.7],
    [19.9, 23.7, 29.1, 31.3, 37.9, 41.1, 47.3, 53.9],
];
const DIFF_MS: [f32; 4] = [4.77, 3.59, 12.73, 9.31];

impl Fdn {
    pub fn new(sr: f32) -> Self {
        let n = (sr * 0.1 * 2.2) as usize; // the longest line at the biggest size, with room to move
        Fdn {
            sr,
            lines: (0..FDN).map(|_| Line::new(n)).collect(),
            lp: [0.0; FDN],
            diff: DIFF_MS.iter().map(|ms| (Line::new((sr * ms / 1000.0 * 2.2) as usize + 8), *ms)).collect(),
            pre: Line::new((sr * 0.21) as usize),
            ph: [0.0, 0.11, 0.23, 0.37, 0.51, 0.63, 0.79, 0.91],
            lc: [Biquad::new(); 2],
            hc: [Biquad::new(); 2],
            f_at: (-1.0, -1.0),
        }
    }

    /// One stereo block, in place. kind 0 room, 1 hall, 2 plate.
    #[allow(clippy::too_many_arguments)]
    pub fn process(&mut self, kind: usize, size: f32, damp: f32, width: f32, mix: f32, dry: f32, predelay: f32, lowcut: f32, highcut: f32, mod_amt: f32,
                   l: &mut [f32], r: &mut [f32]) {
        let sr = self.sr;
        let scale = 0.5 + 1.5 * size;
        let rt60 = match kind {
            0 => 0.25 + 1.6 * size * size,
            1 => 0.8 + 8.0 * size * size,
            _ => 0.5 + 4.5 * size * size,
        };
        let lens: [f32; FDN] = std::array::from_fn(|i| (FDN_MS[kind.min(2)][i] * scale * sr / 1000.0).min(self.lines[i].len() as f32 - 40.0));
        let gains: [f32; FDN] = std::array::from_fn(|i| 10f32.powf(-3.0 * lens[i] / (rt60 * sr)));
        let damp_c = 0.05 + 0.9 * (1.0 - damp);
        let diffusion = if kind == 2 { 0.75 } else { 0.62 };
        let pre = predelay * sr;
        let lc_on = lowcut > 0.0;
        let hc_on = highcut > 0.0;
        if (lowcut, highcut) != self.f_at {
            self.f_at = (lowcut, highcut);
            for b in &mut self.lc {
                b.c = rbj::highpass(lowcut.max(10.0) as f64, 0.7071, sr as f64);
            }
            for b in &mut self.hc {
                b.c = rbj::lowpass(highcut.max(500.0) as f64, 0.7071, sr as f64);
            }
        }
        let depth = mod_amt * sr * 0.0015;
        for k in 0..l.len().min(r.len()) {
            let input = 0.5 * (l[k] + r[k]);
            self.pre.push(input);
            let mut x = if pre >= 1.0 { self.pre.read(pre) } else { input };
            for (line, ms) in self.diff.iter_mut() {
                let d = *ms * scale.min(1.5) * sr / 1000.0;
                let delayed = line.read(d);
                let v = x + delayed * diffusion;
                line.push(v);
                x = delayed - v * diffusion;
            }
            // read the lines, damp them, mix them (Householder), feed back
            let mut outs = [0f32; FDN];
            for i in 0..FDN {
                self.ph[i] = (self.ph[i] + (0.31 + 0.07 * i as f32) / sr) % 1.0;
                let m = depth * (2.0 * std::f32::consts::PI * self.ph[i]).sin();
                let y = self.lines[i].read(lens[i] + m);
                self.lp[i] += (y - self.lp[i]) * damp_c;
                outs[i] = self.lp[i] * gains[i];
            }
            let sum: f32 = outs.iter().sum::<f32>() * (2.0 / FDN as f32);
            for i in 0..FDN {
                let fb = outs[i] - sum;
                self.lines[i].push(fb + x * if i % 2 == 0 { 1.0 } else { -1.0 } * 0.35);
            }
            let mut wl = outs[0] + outs[2] + outs[4] + outs[6] - 0.3 * (outs[1] + outs[5]);
            let mut wr = outs[1] + outs[3] + outs[5] + outs[7] - 0.3 * (outs[2] + outs[6]);
            if lc_on {
                wl = self.lc[0].run(wl);
                wr = self.lc[1].run(wr);
            }
            if hc_on {
                wl = self.hc[0].run(wl);
                wr = self.hc[1].run(wr);
            }
            let (m, s) = (0.5 * (wl + wr), 0.5 * (wl - wr) * width);
            // as loud as the classic reverb at the same mix
            l[k] = l[k] * dry + (m + s) * mix * 2.1;
            r[k] = r[k] * dry + (m - s) * mix * 2.1;
        }
    }

    pub fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        for (l, _) in &mut self.diff {
            l.clear();
        }
        self.pre.clear();
        self.lp = [0.0; FDN];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{kind_index, param_index, SlotCfg};

    fn cfg(kind: &str) -> SlotCfg {
        SlotCfg::new(kind_index(kind).unwrap())
    }
    fn set(c: &mut SlotCfg, key: &str, v: f32) {
        c.p[param_index(c.kind, key).unwrap()] = v;
    }
    fn run(c: &SlotCfg, l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut fx = crate::fx::make(c, 48000.0).unwrap();
        let (mut a, mut b) = (l.to_vec(), r.to_vec());
        for (x, y) in a.chunks_mut(256).zip(b.chunks_mut(256)) {
            fx.process(&c.p, x, y);
        }
        (a, b)
    }
    fn sine(f: f32, a: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| a * (2.0 * std::f32::consts::PI * f * i as f32 / 48000.0).sin()).collect()
    }
    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn deesser_turns_down_loud_highs_only() {
        let c = cfg("deess");
        let (l, _) = run(&c, &sine(8000.0, 0.5, 48000), &sine(8000.0, 0.5, 48000));
        assert!(peak(&l[24000..]) < 0.3, "{}", peak(&l[24000..]));
        let (l, _) = run(&c, &sine(300.0, 0.5, 48000), &sine(300.0, 0.5, 48000));
        assert!((peak(&l[24000..]) - 0.5).abs() < 0.03, "{}", peak(&l[24000..]));
    }

    #[test]
    fn gate_closes_on_quiet_and_opens_on_loud() {
        let c = cfg("gate");
        let (l, _) = run(&c, &sine(200.0, 0.001, 48000), &sine(200.0, 0.001, 48000));
        assert!(peak(&l[24000..]) < 0.0001);
        let (l, _) = run(&c, &sine(200.0, 0.5, 48000), &sine(200.0, 0.5, 48000));
        assert!(peak(&l[24000..]) > 0.45);
    }

    #[test]
    fn imager_zero_width_is_mono_and_utility_flips() {
        let mut c = cfg("width");
        set(&mut c, "width", 0.0);
        let x = sine(500.0, 0.4, 24000);
        let y: Vec<f32> = x.iter().map(|v| -v).collect();
        let (l, r) = run(&c, &x, &y);
        assert!(peak(&l[12000..]) < 0.01 && peak(&r[12000..]) < 0.01, "side-only goes away");
        let mut u = cfg("utility");
        set(&mut u, "phaser", 1.0);
        let (l, r) = run(&u, &x, &y);
        assert!((l[1000] - r[1000]).abs() < 1e-5);
    }

    #[test]
    fn filter_low_pass_keeps_lows_and_cuts_highs() {
        let mut c = cfg("filter");
        set(&mut c, "cutoff", (500f32 / 20.0).ln() / 1000f32.ln());
        let (l, _) = run(&c, &sine(100.0, 0.5, 48000), &sine(100.0, 0.5, 48000));
        assert!(peak(&l[24000..]) > 0.4);
        let (l, _) = run(&c, &sine(8000.0, 0.5, 48000), &sine(8000.0, 0.5, 48000));
        assert!(peak(&l[24000..]) < 0.05, "{}", peak(&l[24000..]));
    }

    #[test]
    fn doubler_and_lofi_and_transient_stay_sane() {
        for k in ["doubler", "lofi", "transient"] {
            let mut c = cfg(k);
            if k == "lofi" {
                set(&mut c, "bits", 0.3);
                set(&mut c, "noise", 0.5);
                set(&mut c, "wow", 0.5);
            }
            let x = sine(220.0, 0.5, 48000);
            let (l, r) = run(&c, &x, &x);
            assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()), "{k}");
            assert!(peak(&l) < 2.0 && peak(&l[24000..]) > 0.1, "{k} {}", peak(&l));
        }
    }

    #[test]
    fn hall_reverb_has_a_long_tail() {
        let mut c = cfg("reverb");
        set(&mut c, "type", 2.0 / 3.0); // hall
        set(&mut c, "size", 0.8);
        let mut x = vec![0f32; 96000];
        x[0] = 1.0;
        let (l, r) = run(&c, &x, &x);
        let tail = peak(&l[48000..60000]).max(peak(&r[48000..60000]));
        assert!(tail > 0.002 && tail < 1.0, "{tail}");
        assert!(l.iter().all(|v| v.is_finite()));
    }
}
