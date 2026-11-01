//! Duet's own plugins: the EQ Pro, Clipper, Multiband, Saturator and Shaper. Each is an Effect
//! (fx.rs) with its knobs as 0..1 values; what the values mean is written next to each one and
//! mirrored in the UI (plugkit.js, PK_SPEC). Built on kit.rs.

use crate::analyzer::Analyzer;
use crate::fx::{Coefs, Effect, Key, Params};
use crate::kit::*;

// --- EQ Pro ------------------------------------------------------------------------------------

pub const EQ_BANDS: usize = 8;
/// knobs per band: on, type, freq, gain, q, slope, ch, dyn, thr
pub const EQ_BP: usize = 9;
pub const EQ_SPEC_N: usize = 64;

/// band shapes, in the order of the `type` knob
#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    Bell,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    Notch,
    BandPass,
}
const SHAPES: [Shape; 7] = [Shape::Bell, Shape::LowShelf, Shape::HighShelf, Shape::LowCut, Shape::HighCut, Shape::Notch, Shape::BandPass];

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EqBand {
    pub on: bool,
    shape: Shape,
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
    /// 12, 24 or 48 dB/octave (cuts)
    slope: usize,
    /// 0 stereo, 1 mid, 2 side, 3 left, 4 right
    ch: usize,
    /// how far the gain moves (dB) when the band's level goes over `thr`
    dynr: f32,
    thr: f32,
}

pub fn eq_band(p: &Params, b: usize) -> EqBand {
    let o = b * EQ_BP;
    EqBand {
        on: p[o] >= 0.5,
        shape: SHAPES[choice(p[o + 1], SHAPES.len())],
        freq: logk(p[o + 2], 20.0, 20000.0),
        gain: -24.0 + 48.0 * p[o + 3],
        q: logk(p[o + 4], 0.1, 18.0),
        slope: [12, 24, 48][choice(p[o + 5], 3)],
        ch: choice(p[o + 6], 5),
        dynr: -24.0 + 48.0 * p[o + 7],
        thr: -60.0 + 60.0 * p[o + 8],
    }
}

/// The filter stages for one band at a gain (the dynamic part moves the gain).
fn band_coefs(b: &EqBand, gain: f32, sr: f32) -> ([Coefs; 4], usize) {
    let (f, q, g, sr) = (b.freq as f64, b.q as f64, gain as f64, sr as f64);
    let mut c = [Coefs::FLAT; 4];
    let cut = |c: &mut [Coefs; 4], hp: bool| -> usize {
        // Butterworth stages; the last one's Q follows the band's Q (resonance)
        let qs: &[f64] = match b.slope {
            12 => &[0.7071],
            24 => &[0.5412, 1.3066],
            _ => &[0.5098, 0.6013, 0.9000, 2.5629],
        };
        let n = qs.len();
        for (i, &bq) in qs.iter().enumerate() {
            let bq = if i == n - 1 { bq * q / 0.7071 } else { bq };
            c[i] = if hp { rbj::highpass(f, bq, sr) } else { rbj::lowpass(f, bq, sr) };
        }
        n
    };
    let n = match b.shape {
        Shape::Bell => {
            c[0] = rbj::peak(f, q, g, sr);
            1
        }
        Shape::LowShelf => {
            c[0] = rbj::shelf(false, f, q.min(4.0), g, sr);
            1
        }
        Shape::HighShelf => {
            c[0] = rbj::shelf(true, f, q.min(4.0), g, sr);
            1
        }
        Shape::LowCut => cut(&mut c, true),
        Shape::HighCut => cut(&mut c, false),
        Shape::Notch => {
            c[0] = rbj::notch(f, q, sr);
            1
        }
        Shape::BandPass => {
            c[0] = rbj::bandpass(f, q, sr);
            1
        }
    };
    (c, n)
}

/// The whole EQ's response in dB at `f` (static, without the dynamic movement), for auto gain.
pub fn eq_pro_db(p: &Params, f: f64, sr: f32) -> f64 {
    (0..EQ_BANDS)
        .map(|i| eq_band(p, i))
        .filter(|b| b.on && b.ch == 0)
        .map(|b| {
            let (c, n) = band_coefs(&b, b.gain, sr);
            c[..n].iter().map(|c| c.db_at(f, sr as f64)).sum::<f64>()
        })
        .sum()
}

struct BandRt {
    cfg: Option<EqBand>,
    c: [Coefs; 4],
    n: usize,
    st: [[Biquad; 4]; 2],
    det: [Biquad; 2],
    env: Env,
    /// the gain it has now (dynamic bands move it)
    now: f32,
}

impl BandRt {
    fn new() -> Self {
        BandRt { cfg: None, c: [Coefs::FLAT; 4], n: 0, st: [[Biquad::new(); 4]; 2], det: [Biquad::new(); 2], env: Env::default(), now: 0.0 }
    }
    #[inline]
    fn run(&mut self, side: usize, x: f32) -> f32 {
        let mut y = x;
        for i in 0..self.n {
            self.st[side][i].c = self.c[i];
            y = self.st[side][i].run(y);
        }
        y
    }
}

pub struct EqPro {
    sr: f32,
    bands: Vec<BandRt>,
    last: Option<Params>,
    out_gain: f32,
    pre: Analyzer,
    post: Analyzer,
    mono: Vec<f32>,
    scope: Vec<f32>,
}

impl EqPro {
    pub fn new(sr: f32) -> Self {
        EqPro {
            sr,
            bands: (0..EQ_BANDS).map(|_| BandRt::new()).collect(),
            last: None,
            out_gain: 1.0,
            pre: Analyzer::with_bands(sr, EQ_SPEC_N, 30.0),
            post: Analyzer::with_bands(sr, EQ_SPEC_N, 30.0),
            mono: vec![0.0; 8192],
            scope: vec![-120.0; 2 * EQ_SPEC_N + EQ_BANDS],
        }
    }

    fn setup(&mut self, p: &Params) {
        if self.last.as_ref() == Some(p) {
            return;
        }
        for (i, b) in self.bands.iter_mut().enumerate() {
            let cfg = eq_band(p, i);
            if b.cfg != Some(cfg) {
                let (c, n) = band_coefs(&cfg, cfg.gain, self.sr);
                b.c = c;
                b.n = if cfg.on { n } else { 0 };
                b.now = cfg.gain;
                let dq = cfg.q.clamp(0.5, 4.0) as f64;
                let det = match cfg.shape {
                    Shape::LowShelf => rbj::lowpass(cfg.freq as f64, 0.7071, self.sr as f64),
                    Shape::HighShelf => rbj::highpass(cfg.freq as f64, 0.7071, self.sr as f64),
                    _ => rbj::bandpass(cfg.freq as f64, dq, self.sr as f64),
                };
                b.det[0].c = det;
                b.det[1].c = det;
                if b.cfg.is_none_or(|o| o.on != cfg.on || o.shape != cfg.shape || o.slope != cfg.slope || o.ch != cfg.ch) {
                    b.st = [[Biquad::new(); 4]; 2];
                }
                b.cfg = Some(cfg);
            }
        }
        let auto = p[EQ_BANDS * EQ_BP + 1] >= 0.5;
        let mut g = -24.0 + 48.0 * p[EQ_BANDS * EQ_BP];
        if auto {
            // turn the output by the curve's average over where most of the music is
            let n = 48;
            let avg: f64 = (0..n).map(|i| eq_pro_db(p, 60.0 * 200f64.powf(i as f64 / (n - 1) as f64), self.sr)).sum::<f64>() / n as f64;
            g -= avg.clamp(-24.0, 24.0) as f32;
        }
        self.out_gain = lin(g);
        self.last = Some(*p);
    }
}

impl Effect for EqPro {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        self.setup(p);
        let n = l.len().min(r.len());
        let sr = self.sr;
        for k in 0..n.min(self.mono.len()) {
            self.mono[k] = 0.5 * (l[k] + r[k]);
        }
        self.pre.process(&self.mono[..n.min(self.mono.len())], &self.mono[..n.min(self.mono.len())]);
        let (att, rel) = (coef(3.0, sr), coef(80.0, sr));
        const SUB: usize = 32;
        let mut at = 0;
        while at < n {
            let m = (n - at).min(SUB);
            for b in self.bands.iter_mut() {
                let Some(cfg) = b.cfg else { continue };
                if !cfg.on || b.n == 0 {
                    continue;
                }
                // a dynamic band: how loud is its part of the sound, and so where its gain is now
                let dynamic = cfg.dynr.abs() > 0.05 && !matches!(cfg.shape, Shape::LowCut | Shape::HighCut);
                if dynamic {
                    let mut lvl = 0f32;
                    for k in at..at + m {
                        let (a, c) = (l[k], r[k]);
                        let x = match cfg.ch {
                            1 => 0.5 * (a + c),
                            2 => 0.5 * (a - c),
                            3 => a,
                            4 => c,
                            _ => 0.5 * (a + c),
                        };
                        let d = b.det[0].run(x).abs();
                        lvl = b.env.run(d, att, rel);
                    }
                    let over = (db(lvl) - cfg.thr).max(0.0);
                    let want = cfg.gain + cfg.dynr * (over / 12.0).min(1.0);
                    if (want - b.now).abs() > 0.02 {
                        b.now = want;
                        let (c, nn) = band_coefs(&cfg, want, sr);
                        b.c = c;
                        b.n = nn;
                    }
                } else if (b.now - cfg.gain).abs() > 1e-4 {
                    b.now = cfg.gain;
                }
                for k in at..at + m {
                    let (a, c) = (l[k], r[k]);
                    match cfg.ch {
                        1 | 2 => {
                            let (mut mid, mut side) = (0.5 * (a + c), 0.5 * (a - c));
                            if cfg.ch == 1 {
                                mid = b.run(0, mid);
                            } else {
                                side = b.run(0, side);
                            }
                            l[k] = mid + side;
                            r[k] = mid - side;
                        }
                        3 => l[k] = b.run(0, a),
                        4 => r[k] = b.run(1, c),
                        _ => {
                            l[k] = b.run(0, a);
                            r[k] = b.run(1, c);
                        }
                    }
                }
            }
            at += m;
        }
        for k in 0..n {
            l[k] *= self.out_gain;
            r[k] *= self.out_gain;
        }
        for k in 0..n.min(self.mono.len()) {
            self.mono[k] = 0.5 * (l[k] + r[k]);
        }
        self.post.process(&self.mono[..n.min(self.mono.len())], &self.mono[..n.min(self.mono.len())]);
        self.scope[..EQ_SPEC_N].copy_from_slice(&self.pre.bands);
        self.scope[EQ_SPEC_N..2 * EQ_SPEC_N].copy_from_slice(&self.post.bands);
        for (i, b) in self.bands.iter().enumerate() {
            self.scope[2 * EQ_SPEC_N + i] = b.now;
        }
    }
    fn reset(&mut self) {
        for b in self.bands.iter_mut() {
            b.st = [[Biquad::new(); 4]; 2];
            b.env = Env::default();
        }
    }
    fn scope(&self) -> &[f32] {
        &self.scope
    }
}

// --- a short delay for the dry signal (so a mix lines up with an oversampled wet) ---------------

struct DryDelay {
    buf: [[f32; 32]; 2],
    pos: usize,
}

impl DryDelay {
    fn new() -> Self {
        DryDelay { buf: [[0.0; 32]; 2], pos: 0 }
    }
    #[inline]
    fn run(&mut self, d: usize, a: f32, b: f32) -> (f32, f32) {
        let d = d.min(31);
        self.buf[0][self.pos] = a;
        self.buf[1][self.pos] = b;
        let i = (self.pos + 32 - d) % 32;
        self.pos = (self.pos + 1) % 32;
        (self.buf[0][i], self.buf[1][i])
    }
}

// --- Clipper -----------------------------------------------------------------------------------

/// input 0..+24 dB, ceiling -24..0 dB, knee 0 (hard) .. 1 (soft), os 1x/2x/4x/8x, mix, output -12..+12 dB,
/// delta (hear only what's cut off)
pub struct Clipper {
    os: Oversampler,
    dry: DryDelay,
    gr: f32,
}

pub const CLIP_OS: usize = 3;

impl Clipper {
    pub fn new(_sr: f32) -> Self {
        Clipper { os: Oversampler::new(), dry: DryDelay::new(), gr: 0.0 }
    }
}

/// Soft above `c·(1-knee)`, never above `c`.
#[inline]
pub fn clip(x: f32, c: f32, knee: f32) -> f32 {
    let a = x.abs();
    let t = c * (1.0 - knee.clamp(0.0, 0.999));
    if a <= t {
        return x;
    }
    let y = if knee <= 0.001 { c } else { t + (c - t) * ((a - t) / (c - t)).tanh() };
    y.min(c).copysign(x)
}

impl Effect for Clipper {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let gin = lin(24.0 * p[0]);
        let ceil = lin(-24.0 + 24.0 * p[1]);
        let knee = p[2];
        let stages = choice(p[CLIP_OS], 4);
        let (mix, gout, delta) = (p[4], lin(-12.0 + 24.0 * p[5]), p[6] >= 0.5);
        let n = l.len().min(r.len());
        let mut dl = [0f32; 4096];
        let mut dr = [0f32; 4096];
        let lat = Oversampler::latency(stages) as usize;
        let mut most = 0f32;
        for k in 0..n {
            l[k] *= gin;
            r[k] *= gin;
            most = most.max(l[k].abs()).max(r[k].abs());
            if k < 4096 {
                let (a, b) = self.dry.run(lat, l[k], r[k]);
                dl[k] = a;
                dr[k] = b;
            }
        }
        self.gr = (db(most) - db(ceil)).max(0.0);
        self.os.run(stages, l, r, |a, b, _| {
            for x in a.iter_mut().chain(b.iter_mut()) {
                *x = clip(*x, ceil, knee);
            }
        });
        for k in 0..n.min(4096) {
            let (wl, wr) = if delta { (dl[k] - l[k], dr[k] - r[k]) } else { (l[k], r[k]) };
            let (xl, xr) = if delta { (wl, wr) } else { (dl[k] + (wl - dl[k]) * mix, dr[k] + (wr - dr[k]) * mix) };
            l[k] = xl * gout;
            r[k] = xr * gout;
        }
    }
    fn gain_reduction(&self) -> f32 {
        self.gr
    }
}

// --- Saturator --------------------------------------------------------------------------------

/// drive 0..36 dB, mode, bias (even harmonics), tone (tilt -6..+6 dB), mix, output -12..+12 dB,
/// os 1x/2x/4x/8x, auto (keep the level)
pub struct Saturator {
    sr: f32,
    os: Oversampler,
    dry: DryDelay,
    dc: [DcBlock; 2],
    tilt: [[Biquad; 2]; 2],
    tilt_at: f32,
    rin: f32,
    rout: f32,
    auto_g: f32,
}

pub const SAT_OS: usize = 6;
pub const SAT_MODES: usize = 5;

impl Saturator {
    pub fn new(sr: f32) -> Self {
        Saturator {
            sr, os: Oversampler::new(), dry: DryDelay::new(), dc: [DcBlock::default(); 2],
            tilt: [[Biquad::new(); 2]; 2], tilt_at: f32::NAN, rin: 0.0, rout: 0.0, auto_g: 1.0,
        }
    }
}

/// The curves: tape, tube, transistor, fuzz, rectify.
#[inline]
pub fn shape(mode: usize, x: f32) -> f32 {
    match mode {
        0 => (2.0 / std::f32::consts::PI) * (std::f32::consts::FRAC_PI_2 * x).atan(),
        1 => {
            let t = x.tanh();
            t + 0.18 * t * t
        }
        2 => x / (1.0 + x.powi(4)).powf(0.25),
        3 => {
            if x > 0.0 {
                (3.0 * x).tanh()
            } else {
                0.8 * (2.2 * x).tanh()
            }
        }
        _ => {
            let t = x.tanh();
            0.5 * t + 0.5 * t.abs()
        }
    }
}

impl Effect for Saturator {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let drive = lin(36.0 * p[0]);
        let mode = choice(p[1], SAT_MODES);
        let bias = 0.6 * p[2];
        let tilt = -6.0 + 12.0 * p[3];
        let (mix, gout) = (p[4], lin(-12.0 + 24.0 * p[5]));
        let stages = choice(p[SAT_OS], 4);
        let auto = p[7] >= 0.5;
        let n = l.len().min(r.len());
        if (tilt - self.tilt_at).abs() > 1e-3 {
            self.tilt_at = tilt;
            let (lo, hi) = (rbj::shelf(false, 800.0, 0.5, -tilt as f64 / 2.0, self.sr as f64), rbj::shelf(true, 800.0, 0.5, tilt as f64 / 2.0, self.sr as f64));
            for ch in &mut self.tilt {
                ch[0].c = lo;
                ch[1].c = hi;
            }
        }
        let lat = Oversampler::latency(stages) as usize;
        let mut dl = [0f32; 4096];
        let mut dr = [0f32; 4096];
        let mut sin = 0f32;
        for k in 0..n.min(4096) {
            let (a, b) = self.dry.run(lat, l[k], r[k]);
            dl[k] = a;
            dr[k] = b;
            sin += l[k] * l[k] + r[k] * r[k];
        }
        let off = shape(mode, bias);
        self.os.run(stages, l, r, |a, b, _| {
            for x in a.iter_mut().chain(b.iter_mut()) {
                *x = shape(mode, *x * drive + bias) - off;
            }
        });
        let mut sout = 0f32;
        for k in 0..n {
            let mut a = self.dc[0].run(l[k]);
            let mut b = self.dc[1].run(r[k]);
            if tilt.abs() > 0.05 {
                let [t0, t1] = &mut self.tilt[0];
                a = t1.run(t0.run(a));
                let [t0, t1] = &mut self.tilt[1];
                b = t1.run(t0.run(b));
            }
            l[k] = a;
            r[k] = b;
            sout += a * a + b * b;
        }
        // auto: the wet as loud as the dry (followed over a third of a second)
        let c = 1.0 - (-(n as f32) / (0.3 * self.sr)).exp();
        self.rin += (sin / n.max(1) as f32 - self.rin) * c;
        self.rout += (sout / n.max(1) as f32 - self.rout) * c;
        let want = if auto && self.rout > 1e-10 { (self.rin / self.rout).sqrt().clamp(0.06, 4.0) } else { 1.0 };
        let g0 = self.auto_g;
        self.auto_g += (want - self.auto_g) * c;
        for k in 0..n.min(4096) {
            let g = g0 + (self.auto_g - g0) * (k as f32 / n as f32);
            l[k] = (dl[k] + (l[k] * g - dl[k]) * mix) * gout;
            r[k] = (dr[k] + (r[k] * g - dr[k]) * mix) * gout;
        }
    }
}

// --- Multiband (upward and downward, the OTT idea) -------------------------------------------

/// depth (how much of the processed sound), time (attack/release x0.1..x10), upward, downward,
/// input -12..+24 dB, output -12..+12 dB, xlow 40..1000 Hz, xhigh 1..12 kHz, then per band (low, mid,
/// high): down threshold -60..0, up threshold -60..0, gain -24..+24 dB
pub struct Multiband {
    sr: f32,
    split: [ThreeWay; 2],
    env: [Env; 3],
    gain: [f32; 3],
    lvl: [f32; 3],
    gr: f32,
    scope: [f32; 6],
}

impl Multiband {
    pub fn new(sr: f32) -> Self {
        Multiband { sr, split: [ThreeWay::default(); 2], env: [Env::default(); 3], gain: [0.0; 3], lvl: [-120.0; 3], gr: 0.0, scope: [-120.0; 6] }
    }
}

impl Effect for Multiband {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let (depth, time, up, down) = (p[0], 10f32.powf((p[1] - 0.5) * 2.0), p[2], p[3]);
        let (gin, gout) = (lin(-12.0 + 36.0 * p[4]), lin(-12.0 + 24.0 * p[5]));
        let (xl, xh) = (logk(p[6], 40.0, 1000.0), logk(p[7], 1000.0, 12000.0));
        for s in &mut self.split {
            s.set(xl, xh, self.sr);
        }
        let band = |b: usize| (-60.0 + 60.0 * p[8 + 3 * b], -60.0 + 60.0 * p[9 + 3 * b], -24.0 + 48.0 * p[10 + 3 * b]);
        let rd = 1.0 + 65.0 * down; // up to 66:1 above
        let ru = 1.0 + 3.0 * up; // up to 4:1 below
        // the lows are slower than the highs
        let times = [(47.0, 282.0), (22.0, 282.0), (13.5, 282.0)];
        let mut most = 0f32;
        let sr = self.sr;
        for k in 0..l.len().min(r.len()) {
            let a = self.split[0].split(l[k] * gin);
            let c = self.split[1].split(r[k] * gin);
            let (mut ol, mut or) = (0f32, 0f32);
            for b in 0..3 {
                let (td, tu, g) = band(b);
                let x = (a[b] * a[b] + c[b] * c[b]) * 0.5;
                let (at, rl) = times[b];
                let e = self.env[b].run(x, coef(at * time, sr), coef(rl * time, sr));
                let lv = 10.0 * e.max(1e-12).log10();
                let red = if lv > td { (lv - td) * (1.0 - 1.0 / rd) } else { 0.0 };
                let boost = if lv < tu && lv > -78.0 { ((tu - lv) * (1.0 - 1.0 / ru)).min(30.0) } else { 0.0 };
                let gdb = -red + boost + g;
                most = most.max(red * depth);
                let gl = lin(gdb);
                let wet_l = a[b] * gl;
                let wet_r = c[b] * gl;
                ol += a[b] + (wet_l - a[b]) * depth;
                or += c[b] + (wet_r - c[b]) * depth;
                self.lvl[b] = lv;
                self.gain[b] = gdb;
            }
            l[k] = ol * gout;
            r[k] = or * gout;
        }
        self.gr = most;
        for b in 0..3 {
            self.scope[2 * b] = self.lvl[b];
            self.scope[2 * b + 1] = self.gain[b];
        }
    }
    fn reset(&mut self) {
        for s in &mut self.split {
            s.reset();
        }
        self.env = [Env::default(); 3];
    }
    fn gain_reduction(&self) -> f32 {
        self.gr
    }
    fn scope(&self) -> &[f32] {
        &self.scope
    }
}

// --- Shaper (volume shaping in time with the song, or on a sidechain) -----------------------------

/// depth, rate (1/32 .. 2 bars), shape (pump, sine, square, saw down, saw up, gate), curve, hold,
/// phase, trigger (tempo / sidechain), threshold -48..0 dB, band (full / lows / highs), xover
/// 40..2000 Hz, mix, smooth 0.5..20 ms
pub struct Shaper {
    sr: f32,
    xo: [Crossover; 2],
    gain: f32,
    ph: f64,
    /// sidechain mode: running since the last hit (None = waiting)
    hit: Option<f64>,
    det: Env,
    armed: bool,
    scope: [f32; 2],
    /// the beat it was told last, and where it is by itself (when nobody moves the song on)
    told: f64,
    own: f64,
}

pub const SHAPER_RATES: [f64; 7] = [0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

impl Shaper {
    pub fn new(sr: f32) -> Self {
        Shaper { sr, xo: [Crossover::default(); 2], gain: 1.0, ph: 0.0, hit: None, det: Env::default(), armed: true, scope: [0.0, 1.0], told: f64::NAN, own: 0.0 }
    }
}

/// The level (0..1 turned down by depth) at `ph` (0..1 through one period).
pub fn shaper_curve(shape: usize, ph: f32, depth: f32, curve: f32, hold: f32) -> f32 {
    let ph = ph.rem_euclid(1.0);
    let down = match shape {
        0 => {
            // pump: all the way down on the beat, held, then back up along the curve
            if ph < hold {
                1.0
            } else {
                let u = ((ph - hold) / (1.0 - hold).max(1e-3)).clamp(0.0, 1.0);
                let e = 0.25 * 16f32.powf(curve); // 0.25 (fast back) .. 4 (slow back)
                1.0 - u.powf(e)
            }
        }
        1 => 0.5 + 0.5 * (2.0 * std::f32::consts::PI * ph).cos(),
        2 => {
            if ph < 0.5 {
                1.0
            } else {
                0.0
            }
        }
        3 => 1.0 - ph,
        4 => ph,
        _ => {
            if ((ph * 4.0) as u32) % 2 == 1 {
                1.0
            } else {
                0.0
            }
        }
    };
    1.0 - depth * down
}

impl Effect for Shaper {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        self.process_keyed(p, l, r, None);
    }
    fn process_keyed(&mut self, p: &Params, l: &mut [f32], r: &mut [f32], key: Key) {
        let depth = p[0];
        let period = SHAPER_RATES[choice(p[1], SHAPER_RATES.len())];
        let shape = choice(p[2], 6);
        let (curve, hold, phase) = (p[3], 0.5 * p[4], p[5] as f64);
        let sidechain = choice(p[6], 2) == 1;
        let thr = lin(-48.0 + 48.0 * p[7]);
        let band = choice(p[8], 3);
        let xf = logk(p[9], 40.0, 2000.0);
        let mix = p[10];
        let sm = coef(logk(p[11], 0.5, 20.0), self.sr);
        for x in &mut self.xo {
            x.set(xf, self.sr);
        }
        let t = song_time();
        let beats_per_sample = t.bpm / 60.0 / self.sr as f64;
        // the beat for this block: the song's, or our own count if it hasn't moved since last time
        let start = if t.beat != self.told { t.beat } else { self.own };
        self.told = t.beat;
        self.own = start + l.len() as f64 * beats_per_sample;
        let (att, rel) = (coef(0.5, self.sr), coef(40.0, self.sr));
        for k in 0..l.len().min(r.len()) {
            let target = if sidechain {
                // a hit on the key (or this sound, without one) starts one period
                let d = match key {
                    Some((kl, kr)) if k < kl.len() => kl[k].abs().max(kr[k].abs()),
                    Some(_) => 0.0,
                    None => l[k].abs().max(r[k].abs()),
                };
                let e = self.det.run(d, att, rel);
                if self.armed && e > thr {
                    self.hit = Some(0.0);
                    self.armed = false;
                } else if e < thr * 0.5 {
                    self.armed = true;
                }
                match self.hit.as_mut() {
                    Some(h) => {
                        let ph = *h / period;
                        *h += beats_per_sample;
                        if ph >= 1.0 {
                            self.hit = None;
                            1.0
                        } else {
                            self.ph = ph;
                            shaper_curve(shape, ph as f32, depth, curve, hold)
                        }
                    }
                    None => 1.0,
                }
            } else {
                let beat = start + k as f64 * beats_per_sample;
                let ph = (beat / period + phase).rem_euclid(1.0);
                self.ph = ph;
                shaper_curve(shape, ph as f32, depth, curve, hold)
            };
            self.gain += (target - self.gain) * sm;
            let g = 1.0 + (self.gain - 1.0) * mix;
            match band {
                1 | 2 => {
                    let (lo_l, hi_l) = self.xo[0].split(l[k]);
                    let (lo_r, hi_r) = self.xo[1].split(r[k]);
                    if band == 1 {
                        l[k] = lo_l * g + hi_l;
                        r[k] = lo_r * g + hi_r;
                    } else {
                        l[k] = lo_l + hi_l * g;
                        r[k] = lo_r + hi_r * g;
                    }
                }
                _ => {
                    l[k] *= g;
                    r[k] *= g;
                }
            }
        }
        self.scope = [self.ph as f32, self.gain];
    }
    fn gain_reduction(&self) -> f32 {
        -db(self.gain)
    }
    fn scope(&self) -> &[f32] {
        &self.scope
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{kind_index, SlotCfg, FX};

    fn sine(f: f32, a: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| a * (2.0 * std::f32::consts::PI * f * i as f32 / 48000.0).sin()).collect()
    }
    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |m, v| m.max(v.abs()))
    }
    fn cfg(kind: &str) -> SlotCfg {
        SlotCfg::new(kind_index(kind).unwrap())
    }
    fn set(c: &mut SlotCfg, key: &str, v: f32) {
        c.p[crate::fx::param_index(c.kind, key).unwrap()] = v;
    }
    fn run(c: &SlotCfg, x: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut fx = crate::fx::make(c, 48000.0).unwrap();
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            fx.process(&c.p, a, b);
        }
        (l, r)
    }

    #[test]
    fn eq_pro_flat_until_a_band_is_on_then_a_bell_where_asked() {
        let c = cfg("eqpro");
        let x = sine(1000.0, 0.5, 48000);
        let (l, _) = run(&c, &x);
        assert!((peak(&l[24000..]) - 0.5).abs() < 0.005);
        let mut c = cfg("eqpro");
        set(&mut c, "b3on", 1.0);
        set(&mut c, "b3freq", (1000f32 / 20.0).ln() / 1000f32.ln());
        set(&mut c, "b3gain", (6.0 + 24.0) / 48.0);
        let (l, _) = run(&c, &x);
        assert!((db(peak(&l[24000..]) / 0.5) - 6.0).abs() < 0.3, "{}", db(peak(&l[24000..]) / 0.5));
        let (l, _) = run(&c, &sine(100.0, 0.5, 48000));
        assert!(db(peak(&l[24000..]) / 0.5).abs() < 0.5);
        // a mid-only band leaves a side-only sound alone
        set(&mut c, "b3ch", 0.25);
        let mut fx = crate::fx::make(&c, 48000.0).unwrap();
        let (mut a, mut b) = (x.clone(), x.iter().map(|v| -v).collect::<Vec<_>>());
        for (p, q) in a.chunks_mut(256).zip(b.chunks_mut(256)) {
            fx.process(&c.p, p, q);
        }
        assert!((peak(&a[24000..]) - 0.5).abs() < 0.01);
    }

    #[test]
    fn eq_pro_dynamic_band_moves_only_when_loud() {
        let mut c = cfg("eqpro");
        set(&mut c, "b3on", 1.0);
        set(&mut c, "b3freq", (1000f32 / 20.0).ln() / 1000f32.ln());
        set(&mut c, "b3dyn", (-12.0 + 24.0) / 48.0); // down to -12 dB when loud
        set(&mut c, "b3thr", (-30.0 + 60.0) / 60.0);
        let (l, _) = run(&c, &sine(1000.0, 0.003, 48000)); // -50 dBFS: below
        assert!(db(peak(&l[24000..]) / 0.003).abs() < 0.5);
        let (l, _) = run(&c, &sine(1000.0, 0.5, 48000));
        assert!(db(peak(&l[24000..]) / 0.5) < -6.0, "{}", db(peak(&l[24000..]) / 0.5));
    }

    #[test]
    fn clipper_never_goes_over_the_ceiling() {
        let mut c = cfg("clipper");
        set(&mut c, "ceiling", 0.75); // -6 dB
        set(&mut c, "os", 0.0);
        let (l, _) = run(&c, &sine(100.0, 1.0, 9600));
        assert!(peak(&l) <= lin(-6.0) + 1e-4, "{}", peak(&l));
        set(&mut c, "os", 1.0); // 8x: the filters may ring a hair over
        let (l, _) = run(&c, &sine(100.0, 1.0, 9600));
        assert!(peak(&l[1000..]) <= lin(-6.0) * 1.03, "{}", peak(&l));
    }

    #[test]
    fn saturator_auto_keeps_the_level() {
        let mut c = cfg("saturate");
        set(&mut c, "drive", 0.6);
        set(&mut c, "auto", 1.0);
        let x = sine(200.0, 0.3, 96000);
        let (l, _) = run(&c, &x);
        let rms = |v: &[f32]| (v.iter().map(|a| a * a).sum::<f32>() / v.len() as f32).sqrt();
        assert!(db(rms(&l[72000..]) / rms(&x[72000..])).abs() < 1.0, "{}", db(rms(&l[72000..]) / rms(&x[72000..])));
    }

    #[test]
    fn multiband_lifts_quiet_and_presses_loud() {
        let mut c = cfg("multi");
        set(&mut c, "depth", 1.0);
        let (l, _) = run(&c, &sine(1000.0, 0.9, 48000));
        let loud = db(peak(&l[24000..]) / 0.9);
        let (l, _) = run(&c, &sine(1000.0, 0.005, 48000));
        let quiet = db(peak(&l[24000..]) / 0.005);
        assert!(loud < -3.0 && quiet > 3.0, "loud {loud} quiet {quiet}");
    }

    #[test]
    fn shaper_pumps_on_the_beat() {
        set_song_time(SongTime { beat: 0.0, bpm: 120.0, playing: true });
        let c = cfg("shaper"); // pump, a quarter note, all the way
        let mut fx = crate::fx::make(&c, 48000.0).unwrap();
        let (mut l, mut r) = (vec![0.5f32; 64], vec![0.5f32; 64]);
        fx.process(&c.p, &mut l, &mut r);
        assert!(l[63] < 0.1, "on the beat {}", l[63]);
        set_song_time(SongTime { beat: 0.9, bpm: 120.0, playing: true });
        let (mut l, mut r) = (vec![0.5f32; 512], vec![0.5f32; 512]);
        fx.process(&c.p, &mut l, &mut r);
        assert!(l[511] > 0.4, "before the next beat {}", l[511]);
        assert_eq!(FX[c.kind].name, "shaper");
    }
}
