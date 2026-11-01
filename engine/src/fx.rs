//! Effects and FX racks. Every mixer strip has a rack: an ordered list of
//! slots, each holding one effect. Parameter names and 0..1 knob ranges are the
//! same as the original pedalboard, so saved settings and the UI carry over.
//!
//! Realtime rule: effect objects are built (allocated) on the control thread
//! and handed to the audio thread in a Box; removed ones are handed back to be
//! freed there. The audio thread only moves Boxes around inside preallocated Vecs.
//! That matters most for NAM: loading a model parses a file and allocates
//! megabytes, all of which happens before the audio thread ever sees it.

use std::f32::consts::PI;

pub const N_KINDS: usize = 24;
pub const NAM: usize = 6;
pub const VST: usize = 7;
pub const EQ: usize = 8;
pub const LIMITER: usize = 9;
pub const COMP: usize = 0;
pub const EQPRO: usize = 10;
pub const CLIPPER: usize = 11;
pub const MULTI: usize = 12;
pub const SATURATE: usize = 13;
pub const SHAPER: usize = 14;
pub const DEESS: usize = 15;
pub const TRANSIENT: usize = 16;
pub const WIDTH: usize = 17;
pub const GATE: usize = 18;
pub const FILTER: usize = 19;
pub const LOFI: usize = 20;
pub const UTILITY: usize = 21;
pub const DOUBLER: usize = 22;
pub const AUTOTUNE: usize = 23;
pub const MAX_PARAMS: usize = 80;
/// what an effect can show in its window besides the meters (spectra, band levels), per slot
pub const SCOPE_N: usize = 136;
pub const MAX_SLOTS: usize = 8;

pub type Params = [f32; MAX_PARAMS];

pub struct FxInfo {
    pub name: &'static str,
    pub params: &'static [&'static str],
    pub defaults: Params,
    /// knobs that jump instead of glide: switches and choices
    pub stepped: &'static [&'static str],
}

/// Effects that can listen to another track (a sidechain).
pub fn takes_key(kind: usize) -> bool {
    matches!(kind, COMP | SHAPER | GATE)
}

/// The first values given, the rest 0.
const fn d<const N: usize>(a: [f32; N]) -> Params {
    let mut out = [0.0; MAX_PARAMS];
    let mut i = 0;
    while i < N {
        out[i] = a[i];
        i += 1;
    }
    out
}

/// EQ Pro: 8 bands of on, type, freq, gain, q, slope, ch, dyn, thr (plugins.rs), then output and auto gain
const EQPRO_PARAMS: [&str; 74] = ["b1on", "b1type", "b1freq", "b1gain", "b1q", "b1slope", "b1ch", "b1dyn", "b1thr", "b2on", "b2type", "b2freq", "b2gain", "b2q", "b2slope", "b2ch", "b2dyn", "b2thr", "b3on", "b3type", "b3freq", "b3gain", "b3q", "b3slope", "b3ch", "b3dyn", "b3thr", "b4on", "b4type", "b4freq", "b4gain", "b4q", "b4slope", "b4ch", "b4dyn", "b4thr", "b5on", "b5type", "b5freq", "b5gain", "b5q", "b5slope", "b5ch", "b5dyn", "b5thr", "b6on", "b6type", "b6freq", "b6gain", "b6q", "b6slope", "b6ch", "b6dyn", "b6thr", "b7on", "b7type", "b7freq", "b7gain", "b7q", "b7slope", "b7ch", "b7dyn", "b7thr", "b8on", "b8type", "b8freq", "b8gain", "b8q", "b8slope", "b8ch", "b8dyn", "b8thr", "output", "auto"];
const EQPRO_DEFAULTS: Params = d([0.0, 0.5, 0.1326, 0.5, 0.3767, 0.5, 0.0, 0.5, 0.6, 0.0, 0.0, 0.2594, 0.5, 0.3767, 0.5, 0.0, 0.5, 0.6, 0.0, 0.0, 0.392, 0.5, 0.3767, 0.5, 0.0, 0.5, 0.6, 0.0, 0.0, 0.534, 0.5, 0.3767, 0.5, 0.0, 0.5, 0.6, 0.0, 0.0, 0.6667, 0.5, 0.3767, 0.5, 0.0, 0.5, 0.6, 0.0, 0.0, 0.7993, 0.5, 0.3767, 0.5, 0.0, 0.5, 0.6, 0.0, 0.0, 0.8997, 0.5, 0.3767, 0.5, 0.0, 0.5, 0.6, 0.0, 0.6667, 0.9677, 0.5, 0.3767, 0.5, 0.0, 0.5, 0.6, 0.5, 0.0]);
const EQPRO_STEPPED: [&str; 33] = ["b1on", "b1type", "b1slope", "b1ch", "b2on", "b2type", "b2slope", "b2ch", "b3on", "b3type", "b3slope", "b3ch", "b4on", "b4type", "b4slope", "b4ch", "b5on", "b5type", "b5slope", "b5ch", "b6on", "b6type", "b6slope", "b6ch", "b7on", "b7type", "b7slope", "b7ch", "b8on", "b8type", "b8slope", "b8ch", "auto"];

pub const FX: [FxInfo; N_KINDS] = [
    // attack 0.1..100 ms, release 10 ms..1 s (the defaults are the old fixed 5 ms / 150 ms)
    // knee 0..24 dB, mix (parallel), schp: the detector's low cut 20..500 Hz (0 = off), char: clean, glue, punch, smooth
    FxInfo { name: "comp", params: &["threshold", "ratio", "makeup", "attack", "release", "knee", "mix", "schp", "char"],
             defaults: d([0.5, 0.5, 0.3, 0.566, 0.588, 0.0, 1.0, 0.0, 0.0]), stepped: &["char"] },
    FxInfo { name: "drive", params: &["drive", "tone", "level"], defaults: d([0.4, 0.5, 0.5]), stepped: &[] },
    FxInfo { name: "chorus", params: &["rate", "depth", "mix"], defaults: d([0.3, 0.5, 0.5]), stepped: &[] },
    // stereo: 0 = both sides together (tremolo) .. 1 = opposite (auto-pan)
    FxInfo { name: "tremolo", params: &["rate", "depth", "stereo"], defaults: d([0.4, 0.6, 0.0]), stepped: &[] },
    // dry 1 = the dry signal at full level (as on a channel); 0 for a send bus
    // sync: free (time) or a note value in time with the song; low/high cut on the echoes (0 / 1 = off),
    // drive: grit in the repeats, wow: tape wobble, duck: echoes step back while you play
    FxInfo { name: "delay", params: &["time", "feedback", "mix", "pingpong", "dry", "sync", "lowcut", "highcut", "drive", "wow", "duck"],
             defaults: d([0.35, 0.4, 0.35, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]), stepped: &["sync"] },
    // type: classic, room, hall, plate; predelay 0..200 ms; low/high cut on the tail; mod: movement in the tail
    FxInfo { name: "reverb", params: &["size", "damp", "mix", "width", "dry", "type", "predelay", "lowcut", "highcut", "mod"],
             defaults: d([0.6, 0.4, 0.3, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.3]), stepped: &["type"] },
    // input/output: -12..+12 dB around the middle of the knob
    FxInfo { name: "nam", params: &["input", "output"], defaults: d([0.5, 0.5]), stepped: &[] },
    // a VST3 plugin; its own controls live in its own window
    FxInfo { name: "vst", params: &["mix"], defaults: d([1.0]), stepped: &[] },
    // gains -15..+15 dB in the middle of the knob; lowcut 0 / highcut 1 = off
    FxInfo { name: "eq", params: &["lowcut", "low", "lowfreq", "mid", "midfreq", "q", "high", "highfreq", "highcut"],
             defaults: d([0.0, 0.5, 0.402, 0.5, 0.5, 0.343, 0.5, 0.727, 1.0]), stepped: &[] },
    // gain 0..+24 dB in, ceiling -12..0 dB, release 10 ms..1 s
    // link: 0 each side on its own .. 1 both together; tp: true peak (between the samples too)
    FxInfo { name: "limiter", params: &["gain", "ceiling", "release", "link", "tp"], defaults: d([0.0, 0.992, 0.4, 1.0, 1.0]), stepped: &["tp"] },
    FxInfo { name: "eqpro", params: &EQPRO_PARAMS, defaults: EQPRO_DEFAULTS, stepped: &EQPRO_STEPPED },
    FxInfo { name: "clipper", params: &["input", "ceiling", "knee", "os", "mix", "output", "delta"],
             defaults: d([0.0, 1.0, 0.3, 0.667, 1.0, 0.5, 0.0]), stepped: &["os", "delta"] },
    FxInfo { name: "multi", params: &["depth", "time", "upward", "downward", "input", "output", "xlow", "xhigh",
             "lowdown", "lowup", "lowgain", "middown", "midup", "midgain", "highdown", "highup", "highgain"],
             defaults: d([0.5, 0.5, 1.0, 1.0, 0.3333, 0.5, 0.2493, 0.3697, 0.5, 0.3333, 0.5, 0.5, 0.3333, 0.5, 0.5, 0.3333, 0.5]), stepped: &[] },
    FxInfo { name: "saturate", params: &["drive", "mode", "bias", "tone", "mix", "output", "os", "auto"],
             defaults: d([0.25, 0.0, 0.0, 0.5, 1.0, 0.5, 0.333, 1.0]), stepped: &["mode", "os", "auto"] },
    FxInfo { name: "shaper", params: &["depth", "rate", "shape", "curve", "hold", "phase", "trig", "thresh", "band", "xover", "mix", "smooth"],
             defaults: d([1.0, 0.5, 0.0, 0.5, 0.1, 0.0, 0.0, 0.5, 0.0, 0.3486, 1.0, 0.0]), stepped: &["rate", "shape", "trig", "band"] },
    FxInfo { name: "deess", params: &["freq", "thresh", "range", "mode", "listen"], defaults: d([0.6578, 0.5, 0.4167, 0.0, 0.0]), stepped: &["mode", "listen"] },
    FxInfo { name: "transient", params: &["attack", "sustain", "speed", "output", "mix"], defaults: d([0.5, 0.5, 0.5, 0.5, 1.0]), stepped: &[] },
    FxInfo { name: "width", params: &["width", "lowwidth", "highwidth", "xover", "monobass", "balance", "output"],
             defaults: d([0.5, 0.5, 0.5, 0.4307, 0.0, 0.5, 0.5]), stepped: &[] },
    FxInfo { name: "gate", params: &["thresh", "range", "attack", "hold", "release", "hyst", "schp", "sclp", "listen"],
             defaults: d([0.5, 1.0, 0.3333, 0.04, 0.5304, 0.3333, 0.0, 1.0, 0.0]), stepped: &["listen"] },
    FxInfo { name: "filter", params: &["type", "slope", "cutoff", "reso", "drive", "rate", "shape", "lfo", "env", "mix"],
             defaults: d([0.0, 0.0, 0.6, 0.3, 0.0, 0.5, 0.0, 0.5, 0.5, 1.0]), stepped: &["type", "slope", "rate", "shape"] },
    FxInfo { name: "lofi", params: &["bits", "rate", "wow", "flutter", "noise", "tone", "mix", "output"],
             defaults: d([0.5, 0.6, 0.2, 0.1, 0.15, 0.7, 1.0, 0.5]), stepped: &[] },
    FxInfo { name: "utility", params: &["gain", "pan", "width", "phasel", "phaser", "mono", "swap", "dc"],
             defaults: d([0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]), stepped: &["phasel", "phaser", "mono", "swap", "dc"] },
    FxInfo { name: "doubler", params: &["detune", "delay", "width", "voices", "mix", "lowcut"],
             defaults: d([0.24, 0.4, 0.8, 0.0, 0.5, 0.0]), stepped: &["voices"] },
    // key C..B, scale (chromatic, major, minor, harmonic minor, pentatonics, blues), speed 0..400 ms,
    // humanize, amount, transpose -12..+12, mix
    FxInfo { name: "autotune", params: &["key", "scale", "speed", "humanize", "amount", "transpose", "mix"],
             defaults: d([0.0, 1.0 / 6.0, 0.25, 0.3, 1.0, 0.5, 1.0]), stepped: &["key", "scale", "transpose"] },
];

/// Tempo for tempo-synced plugins: the metronome's, written by the audio thread.
pub static TEMPO_BITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x4059000000000000); // 100.0

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
        ("eq", &[("lowcut", 0.28), ("mid", 0.44), ("midfreq", 0.35), ("high", 0.6)]),
        ("comp", &[("threshold", 0.35), ("ratio", 0.5), ("makeup", 0.35)]),
        ("reverb", &[("size", 0.55), ("damp", 0.45), ("mix", 0.25)]),
    ]),
    ("Surf", &[
        ("tremolo", &[("rate", 0.45), ("depth", 0.5)]),
        ("reverb", &[("size", 0.75), ("damp", 0.2), ("mix", 0.5)]),
    ]),
];

/// What we know about a loaded NAM model, for the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo {
    pub name: String,
    pub sample_rate: f64,
    pub includes_cab: Option<bool>,
    pub loudness: Option<f32>,
}

/// What a slot holds, without the DSP state: the control side's mirror.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotCfg {
    pub kind: usize,
    pub on: bool,
    pub p: Params,
    /// NAM only: the .nam file, and what's in it (None until one is loaded)
    pub model: Option<String>,
    pub info: Option<ModelInfo>,
    /// why the model couldn't be loaded, if it couldn't
    pub error: Option<String>,
    /// VST only: which plugin, and its saved state
    pub vst: Option<VstRef>,
    /// compressor only: the song track (slot) it listens to instead of its input
    pub key: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct VstRef {
    pub path: String,
    pub cid: String,
    pub name: String,
    /// encoded plugin state (see vst::State); refreshed when the mixer is saved
    pub state: Option<String>,
    /// filled in once built
    pub id: u64,
    pub latency: u32,
    pub has_editor: bool,
}

impl SlotCfg {
    pub fn new(kind: usize) -> Self {
        SlotCfg { kind, on: true, p: FX[kind].defaults, model: None, info: None, error: None, vst: None, key: None }
    }

    /// Samples this slot delays the sound by (switched off: none).
    pub fn latency(&self, sr: f32) -> u32 {
        if !self.on {
            return 0;
        }
        match self.kind {
            VST => self.vst.as_ref().filter(|_| self.error.is_none()).map_or(0, |v| v.latency),
            LIMITER => Limiter::lookahead(sr) as u32,
            CLIPPER => crate::kit::Oversampler::latency(crate::kit::choice(self.p[crate::plugins::CLIP_OS], 4)),
            SATURATE => crate::kit::Oversampler::latency(crate::kit::choice(self.p[crate::plugins::SAT_OS], 4)),
            AUTOTUNE => crate::plugins3::latency(sr),
            _ => 0,
        }
    }
}

/// What a rack delays the sound by, in samples.
pub fn rack_latency(rack: &[SlotCfg], sr: f32) -> u32 {
    rack.iter().map(|s| s.latency(sr)).sum()
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

/// A sidechain signal: another track's sound (left, right).
pub type Key<'a> = Option<(&'a [f32], &'a [f32])>;

pub trait Effect: Send {
    /// One stereo block, in place.
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]);
    /// The same, listening to a sidechain signal (effects that can).
    fn process_keyed(&mut self, p: &Params, l: &mut [f32], r: &mut [f32], _key: Key) {
        self.process(p, l, r)
    }
    fn reset(&mut self) {}
    /// How much it turned the sound down in the last block, in dB (compressors, limiters).
    fn gain_reduction(&self) -> f32 {
        0.0
    }
    /// What its window draws besides the meters (at most SCOPE_N values: spectra, band levels).
    fn scope(&self) -> &[f32] {
        &[]
    }
    /// Effects that can follow a sample-rate change in place (keeping their
    /// state) say so; the rest are rebuilt.
    fn set_sample_rate(&mut self, _sr: f32) -> bool {
        false
    }
}

/// A VST3 plugin in a rack slot.
pub struct VstEffect {
    pub proc: crate::vst::VstProc,
}

impl Effect for VstEffect {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let tempo = f64::from_bits(TEMPO_BITS.load(std::sync::atomic::Ordering::Relaxed));
        self.proc.process(l, r, p[0], tempo);
    }
    fn set_sample_rate(&mut self, sr: f32) -> bool {
        self.proc.set_sample_rate(sr as f64);
        true
    }
}

/// Build an effect. Allocates (and for NAM reads + parses the model file), so
/// never call this on the audio thread.
pub fn make(cfg: &SlotCfg, sr: f32) -> Result<Box<dyn Effect>, String> {
    Ok(match cfg.kind {
        0 => Box::new(Compressor::new(sr)),
        1 => Box::new(Drive::new(sr)),
        2 => Box::new(Chorus::new(sr)),
        3 => Box::new(Tremolo { phase: 0.0, sr }),
        4 => Box::new(Delay::new(sr)),
        5 => Box::new(Reverb::new(sr)),
        NAM => Box::new(match &cfg.model {
            Some(path) => NamAmp::load(path, sr)?.0,
            None => NamAmp::empty(sr),
        }),
        EQ => Box::new(Eq::new(sr)),
        LIMITER => Box::new(Limiter::new(sr)),
        EQPRO => Box::new(crate::plugins::EqPro::new(sr)),
        CLIPPER => Box::new(crate::plugins::Clipper::new(sr)),
        MULTI => Box::new(crate::plugins::Multiband::new(sr)),
        SATURATE => Box::new(crate::plugins::Saturator::new(sr)),
        SHAPER => Box::new(crate::plugins::Shaper::new(sr)),
        DEESS => Box::new(crate::plugins2::DeEsser::new(sr)),
        TRANSIENT => Box::new(crate::plugins2::Transient::new(sr)),
        WIDTH => Box::new(crate::plugins2::Imager::new(sr)),
        GATE => Box::new(crate::plugins2::Gate::new(sr)),
        FILTER => Box::new(crate::plugins2::Filter::new(sr)),
        LOFI => Box::new(crate::plugins2::LoFi::new(sr)),
        UTILITY => Box::new(crate::plugins2::Utility::new(sr)),
        DOUBLER => Box::new(crate::plugins2::Doubler::new(sr)),
        AUTOTUNE => Box::new(crate::plugins3::AutoTune::new(sr)),
        _ => {
            let v = cfg.vst.as_ref().ok_or("no plugin chosen")?;
            Box::new(VstEffect { proc: crate::vst::VstProc::create(&v.path, &v.cid, v.state.as_deref(), sr as f64)? })
        }
    })
}

/// A slot as the audio thread holds it: settings + the effect's DSP state.
pub struct Slot {
    pub cfg: SlotCfg,
    pub fx: Box<dyn Effect>,
    /// the knobs as the effect hears them: following cfg.p over ~15 ms, so a turn (or automation)
    /// doesn't zipper
    sm: Params,
    /// which knobs jump (FxInfo::stepped), a bit each
    stepped: u128,
    fresh: bool,
    sr: f32,
    /// for the plugin's window: peaks in and out, and how much it turns down (dB)
    pub meter: SlotMeter,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SlotMeter {
    pub input: f32,
    pub output: f32,
    pub gr: f32,
}


impl Slot {
    /// For a NAM slot with a model this also fills in `cfg.info`.
    pub fn new(mut cfg: SlotCfg, sr: f32) -> Result<Box<Slot>, String> {
        if cfg.kind == NAM {
            if let Some(path) = cfg.model.clone() {
                let (amp, info) = NamAmp::load(&path, sr)?;
                cfg.info = Some(info);
                return Ok(Slot::wrap(cfg, Box::new(amp), sr));
            }
        }
        if cfg.kind == VST {
            let v = cfg.vst.clone().ok_or("no plugin chosen")?;
            let proc = crate::vst::VstProc::create(&v.path, &v.cid, v.state.as_deref(), sr as f64)?;
            let inst = proc.info();
            cfg.vst = Some(VstRef {
                name: if inst.name.is_empty() { v.name } else { inst.name.clone() },
                id: proc.id(),
                latency: inst.latency,
                has_editor: inst.has_editor,
                ..v
            });
            return Ok(Slot::wrap(cfg, Box::new(VstEffect { proc }), sr));
        }
        let fx = make(&cfg, sr)?;
        Ok(Slot::wrap(cfg, fx, sr))
    }

    /// A slot whose effect couldn't be built (a plugin that isn't installed
    /// here, a model file that's gone): it lets the sound through untouched,
    /// keeps its settings so it can be retried, and says what went wrong.
    pub fn placeholder(mut cfg: SlotCfg, error: String) -> Box<Slot> {
        cfg.error = Some(error);
        Slot::wrap(cfg, Box::new(Bypass), 48000.0)
    }

    pub fn wrap(cfg: SlotCfg, fx: Box<dyn Effect>, sr: f32) -> Box<Slot> {
        let info = &FX[cfg.kind.min(N_KINDS - 1)];
        let stepped = info.params.iter().enumerate().filter(|(_, n)| info.stepped.contains(n)).fold(0u128, |m, (i, _)| m | 1 << i);
        Box::new(Slot { sm: cfg.p, cfg, fx, stepped, fresh: true, sr, meter: SlotMeter::default() })
    }

    /// One block through the effect (on or off), with the knobs smoothed and the meters kept.
    fn run<'k>(&mut self, l: &mut [f32], r: &mut [f32], keys: &dyn Fn(usize) -> Key<'k>) {
        let n = l.len();
        let pk = |l: &[f32], r: &[f32]| l.iter().chain(r.iter()).fold(0f32, |a, v| a.max(v.abs()));
        let decay = (-(n as f32) / (0.3 * self.sr)).exp(); // meters fall over ~0.3 s
        let inp = pk(l, r);
        if !self.cfg.on {
            self.meter = SlotMeter { input: inp.max(self.meter.input * decay), output: inp.max(self.meter.output * decay), gr: 0.0 };
            return;
        }
        if self.fresh {
            self.sm = self.cfg.p;
            self.fresh = false;
        } else {
            let a = 1.0 - (-(n as f32) / (0.015 * self.sr)).exp();
            let stepped = self.stepped;
            for i in 0..MAX_PARAMS {
                if stepped & (1 << i) != 0 {
                    self.sm[i] = self.cfg.p[i];
                } else {
                    self.sm[i] += (self.cfg.p[i] - self.sm[i]) * a;
                    if (self.sm[i] - self.cfg.p[i]).abs() < 1e-5 {
                        self.sm[i] = self.cfg.p[i];
                    }
                }
            }
        }
        match self.cfg.key {
            Some(k) => self.fx.process_keyed(&self.sm, l, r, keys(k)),
            None => self.fx.process(&self.sm, l, r),
        }
        let out = pk(l, r);
        self.meter = SlotMeter {
            input: inp.max(self.meter.input * decay),
            output: out.max(self.meter.output * decay),
            gr: self.fx.gain_reduction().max(self.meter.gr * decay),
        };
    }
}

struct Bypass;

impl Effect for Bypass {
    fn process(&mut self, _p: &Params, _l: &mut [f32], _r: &mut [f32]) {}
}

pub struct Rack {
    pub slots: Vec<Box<Slot>>,
}

impl Rack {
    pub fn new() -> Self {
        Rack { slots: Vec::with_capacity(MAX_SLOTS) }
    }

    /// One stereo block through every slot that's on. `keys` finds a song
    /// track's sound for a compressor that listens to one (sidechain).
    pub fn process<'k>(&mut self, l: &mut [f32], r: &mut [f32], keys: &dyn Fn(usize) -> Key<'k>) {
        for s in self.slots.iter_mut() {
            s.run(l, r, keys);
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

    /// Swap a slot for a new one (e.g. a NAM slot with a different model); the
    /// old one comes back so it can be freed elsewhere.
    pub fn replace(&mut self, pos: usize, slot: Box<Slot>) -> Box<Slot> {
        match self.slots.get_mut(pos) {
            Some(s) => std::mem::replace(s, slot),
            None => slot,
        }
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

/// No sidechain anywhere (racks outside the song).
pub fn no_keys(_: usize) -> Key<'static> {
    None
}

// --- NAM amp -----------------------------------------------------------------------

/// A Neural Amp Modeler capture. The model itself (nam-rs) only does the
/// network's forward pass; like the reference plugin we add input/output trim,
/// loudness normalisation (to -18 dB, so swapping models doesn't jump in
/// level) and a DC blocker after it.
pub struct NamAmp {
    model: Option<nam_rs::Model>,
    norm: f32,
    dc_x: f32,
    dc_y: f32,
    dc_r: f32,
}

impl NamAmp {
    fn empty(sr: f32) -> Self {
        NamAmp { model: None, norm: 1.0, dc_x: 0.0, dc_y: 0.0, dc_r: 1.0 - 2.0 * PI * 10.0 / sr }
    }

    pub fn load(path: &str, sr: f32) -> Result<(Self, ModelInfo), String> {
        let file = nam_rs::NamModel::from_file(path).map_err(|e| format!("couldn't read {path}: {e}"))?;
        let model = nam_rs::Model::from_nam(&file).map_err(|e| format!("unsupported model: {e}"))?;
        let meta = file.metadata_typed();
        let name = meta.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        });
        let loudness = file.loudness();
        let info = ModelInfo { name, sample_rate: file.expected_sample_rate(), includes_cab: file.includes_cab(), loudness };
        let mut amp = NamAmp::empty(sr);
        amp.norm = loudness.map_or(1.0, |l| db_to_lin(-18.0 - l));
        amp.model = Some(model);
        Ok((amp, info))
    }
}

impl Effect for NamAmp {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let Some(model) = self.model.as_mut() else { return };
        // an amp is mono: it plays the sum, the same on both sides
        let gin = db_to_lin(-12.0 + 24.0 * p[0]);
        let gout = db_to_lin(-12.0 + 24.0 * p[1]) * self.norm;
        for (a, b) in l.iter_mut().zip(r.iter()) {
            *a = 0.5 * (*a + *b) * gin;
        }
        model.process_buffer(l);
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let y = *a - self.dc_x + self.dc_r * self.dc_y;
            self.dc_x = *a;
            self.dc_y = y;
            *a = y * gout;
            *b = *a;
        }
    }
    fn reset(&mut self) {
        if let Some(m) = self.model.as_mut() {
            m.reset();
        }
        self.dc_x = 0.0;
        self.dc_y = 0.0;
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
    sr: f32,
    gr: f32,
    /// the gain it applies now (punch: the detector listens to the output)
    g: f32,
    hp: [crate::kit::Biquad; 2],
    hp_at: f32,
}

impl Compressor {
    fn new(sr: f32) -> Self {
        Compressor { env: 0.0, sr, gr: 0.0, g: 1.0, hp: [crate::kit::Biquad::new(); 2], hp_at: -1.0 }
    }
}

/// How much to turn down (dB) at `over` dB above the threshold, with a soft knee `w` dB wide.
#[inline]
fn comp_curve(over: f32, ratio: f32, w: f32) -> f32 {
    let s = 1.0 - 1.0 / ratio;
    if w > 0.01 && 2.0 * over.abs() <= w {
        s * (over + w / 2.0).powi(2) / (2.0 * w)
    } else if over > 0.0 {
        s * over
    } else {
        0.0
    }
}

impl Effect for Compressor {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        self.process_keyed(p, l, r, None);
    }
    /// Stereo-linked: both sides get the same gain, from the louder one (or
    /// from the key: a kick that pushes the bass down). Characters: clean (peak), glue (RMS, a
    /// bus compressor), punch (listens to its own output, fast, a little grit), smooth (lets go
    /// slower the more it's pressing, like an optical one).
    fn process_keyed(&mut self, p: &Params, l: &mut [f32], r: &mut [f32], key: Key) {
        let thr_db = -40.0 + 36.0 * p[0]; // -40 .. -4 dBFS
        let ratio = 1.0 + 11.0 * p[1]; // 1:1 .. 12:1
        let makeup = 18.0 * p[2];
        let character = crate::kit::choice(p[8], 4);
        let mut att_s = 0.0001 * 1000f32.powf(p[3]); // 0.1 .. 100 ms
        let rel_s = 0.01 * 100f32.powf(p[4]); // 10 ms .. 1 s
        if character == 3 {
            att_s = att_s.max(0.01);
        }
        let knee = 24.0 * p[5];
        let mix = p[6];
        let sc_on = p[7] > 0.005;
        if sc_on {
            let f = 20.0 * 25f32.powf(p[7]);
            if (f - self.hp_at).abs() > 0.01 {
                self.hp_at = f;
                let c = crate::kit::rbj::highpass(f as f64, 0.7071, self.sr as f64);
                self.hp[0].c = c;
                self.hp[1].c = c;
            }
        }
        let att = 1.0 - (-1.0 / (att_s * self.sr)).exp();
        let mut most = 0f32;
        for k in 0..l.len() {
            let (mut a, mut b) = match key {
                Some((kl, kr)) if k < kl.len() => (kl[k], kr[k]),
                Some(_) => (0.0, 0.0),
                None if character == 2 => (l[k] * self.g, r[k] * self.g),
                None => (l[k], r[k]),
            };
            if sc_on {
                a = self.hp[0].run(a);
                b = self.hp[1].run(b);
            }
            let det = if character == 1 { 0.5 * (a * a + b * b) } else { a.abs().max(b.abs()) };
            // smooth: the deeper it presses, the slower it lets go
            let rel_now = if character == 3 { rel_s * (1.0 + self.gr_now() / 4.0) } else { rel_s };
            let rel = 1.0 - (-1.0 / (rel_now * self.sr)).exp();
            let c = if det > self.env { att } else { rel };
            self.env += (det - self.env) * c;
            let env_db = if character == 1 { 10.0 * (self.env + 1e-18).log10() } else { 20.0 * (self.env + 1e-9).log10() };
            let red = comp_curve(env_db - thr_db, ratio, knee);
            most = most.max(red);
            self.g = db_to_lin(-red);
            let g = self.g * db_to_lin(makeup);
            let (mut wl, mut wr) = (l[k] * g, r[k] * g);
            if character == 2 && red > 0.0 {
                // a touch of grit while it works
                wl = (wl * 1.1).tanh() / 1.1;
                wr = (wr * 1.1).tanh() / 1.1;
            }
            l[k] += (wl - l[k]) * mix;
            r[k] += (wr - r[k]) * mix;
        }
        self.gr = most;
    }
    fn gain_reduction(&self) -> f32 {
        self.gr
    }
}

impl Compressor {
    #[inline]
    fn gr_now(&self) -> f32 {
        -20.0 * self.g.max(1e-6).log10()
    }
}

struct Drive {
    lp: [f32; 2],
    coef: f32,
}

impl Drive {
    fn new(sr: f32) -> Self {
        Drive { lp: [0.0; 2], coef: 1.0 - (-2.0 * PI * 1800.0 / sr).exp() }
    }
}

impl Effect for Drive {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let g = 1.0 + 40.0 * p[0] * p[0];
        let norm = 1.0 / g.tanh().sqrt();
        let (tone, level) = (p[1], 0.2 + 1.3 * p[2]);
        for (ch, buf) in [l, r].into_iter().enumerate() {
            let lp = &mut self.lp[ch];
            for x in buf.iter_mut() {
                let y = (*x * g).tanh() * norm;
                *lp += (y - *lp) * self.coef;
                *x = (*lp * (1.0 - tone) + y * tone) * level;
            }
        }
    }
}

struct Chorus {
    ring: [Ring; 2],
    phase: f32,
    sr: f32,
}

impl Chorus {
    fn new(sr: f32) -> Self {
        Chorus { ring: [Ring::new((sr * 0.05) as usize), Ring::new((sr * 0.05) as usize)], phase: 0.0, sr }
    }
}

impl Effect for Chorus {
    /// Two voices per side, the right side's a quarter turn on: a mono
    /// signal comes out wide.
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let rate = 0.1 + 3.0 * p[0];
        let inc = 2.0 * PI * rate / self.sr;
        let base = 0.012 * self.sr;
        let depth = 0.006 * self.sr * p[1];
        let m = p[2];
        for k in 0..l.len() {
            for (ch, x) in [&mut l[k], &mut r[k]].into_iter().enumerate() {
                let ring = &mut self.ring[ch];
                ring.write(*x);
                let ph = self.phase + ch as f32 * PI / 2.0;
                let d1 = base + depth * ph.sin();
                let d2 = base + depth * (ph + PI).sin();
                let wet = 0.5 * (ring.tap_frac(d1) + ring.tap_frac(d2));
                *x = *x * (1.0 - 0.5 * m) + wet * m;
            }
            self.phase = (self.phase + inc) % (2.0 * PI);
        }
    }
    fn reset(&mut self) {
        for r in self.ring.iter_mut() {
            r.clear();
        }
    }
}

struct Tremolo {
    phase: f32,
    sr: f32,
}

impl Effect for Tremolo {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let rate = 0.5 + 11.5 * p[0];
        let inc = 2.0 * PI * rate / self.sr;
        let spread = PI * p[2]; // the right side's wave this far behind: at 1, an auto-pan
        for k in 0..l.len() {
            l[k] *= 1.0 - p[1] * (0.5 + 0.5 * self.phase.sin());
            r[k] *= 1.0 - p[1] * (0.5 + 0.5 * (self.phase + spread).sin());
            self.phase = (self.phase + inc) % (2.0 * PI);
        }
    }
}

struct Delay {
    ring: [Ring; 2],
    lp: [f32; 2],
    sr: f32,
    cut: [[crate::kit::Biquad; 2]; 2], // [side][low cut, high cut]
    cut_at: (f32, f32),
    wow: f32,
    duck: crate::kit::Env,
}

/// note values for a synced delay, in beats (after "free")
pub const DELAY_NOTES: [f64; 12] = [0.125, 1.0 / 6.0, 0.25, 0.375, 1.0 / 3.0, 0.5, 0.75, 2.0 / 3.0, 1.0, 1.5, 2.0, 4.0];

impl Delay {
    fn new(sr: f32) -> Self {
        let n = (sr * 4.1) as usize;
        Delay { ring: [Ring::new(n), Ring::new(n)], lp: [0.0; 2], sr, cut: [[crate::kit::Biquad::new(); 2]; 2], cut_at: (-1.0, -1.0), wow: 0.0,
            duck: crate::kit::Env::default() }
    }
}

impl Effect for Delay {
    /// Ping-pong 0: each side echoes itself. 1: the echoes bounce left, right,
    /// left (the input goes in on the left).
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let sync = crate::kit::choice(p[5], DELAY_NOTES.len() + 1);
        let secs = if sync == 0 {
            0.06 + 1.1 * p[0] as f64
        } else {
            DELAY_NOTES[sync - 1] * 60.0 / crate::kit::song_time().bpm.max(20.0)
        };
        let max = (self.ring[0].buf.len() - 4) as f32;
        let d = ((secs * self.sr as f64) as f32).clamp(2.0, max - 200.0);
        let fb = 0.92 * p[1];
        let (mix, pp, dry) = (p[2], p[3], p[4]);
        let (lc_on, hc_on) = (p[6] > 0.005, p[7] < 0.995);
        let (lcf, hcf) = (20.0 * 100f32.powf(p[6]), 1000.0 * 20f32.powf(p[7]));
        if (lcf, hcf) != self.cut_at {
            self.cut_at = (lcf, hcf);
            for side in &mut self.cut {
                side[0].c = crate::kit::rbj::highpass(lcf as f64, 0.7071, self.sr as f64);
                side[1].c = crate::kit::rbj::lowpass(hcf as f64, 0.7071, self.sr as f64);
            }
        }
        let drive = 1.0 + 6.0 * p[8];
        let wow = p[9] * self.sr * 0.0025;
        let duck = p[10];
        let (da, dr) = (crate::kit::coef(5.0, self.sr), crate::kit::coef(250.0, self.sr));
        for k in 0..l.len() {
            self.wow = (self.wow + 0.6 / self.sr) % 1.0;
            let dd = d + wow * (2.0 * PI * self.wow).sin();
            let mut el = self.ring[0].tap_frac(dd);
            let mut er = self.ring[1].tap_frac(dd);
            self.lp[0] += (el - self.lp[0]) * 0.5; // repeats get darker, like tape
            self.lp[1] += (er - self.lp[1]) * 0.5;
            el = self.lp[0];
            er = self.lp[1];
            if lc_on {
                el = self.cut[0][0].run(el);
                er = self.cut[1][0].run(er);
            }
            if hc_on {
                el = self.cut[0][1].run(el);
                er = self.cut[1][1].run(er);
            }
            if p[8] > 0.001 {
                el = (el * drive).tanh() / drive.sqrt();
                er = (er * drive).tanh() / drive.sqrt();
            }
            let mono = 0.5 * (l[k] + r[k]);
            let in_l = l[k] + (mono - l[k]) * pp;
            let in_r = r[k] * (1.0 - pp);
            self.ring[0].write(in_l + (el + (er - el) * pp) * fb);
            self.ring[1].write(in_r + (er + (el - er) * pp) * fb);
            // ducking: the echoes step back while the input is loud
            let e = self.duck.run(l[k].abs().max(r[k].abs()), da, dr);
            let g = mix * (1.0 - duck * (e * 4.0).min(1.0));
            l[k] = l[k] * dry + el * g;
            r[k] = r[k] * dry + er * g;
        }
    }
    fn reset(&mut self) {
        for r in self.ring.iter_mut() {
            r.clear();
        }
        self.lp = [0.0; 2];
    }
}

/// Freeverb: eight combs and four all-passes per side, the right side's a
/// little longer, so the tail comes out wide.
struct Reverb {
    combs: [Vec<(Ring, usize, f32)>; 2], // ring, delay, filter store
    aps: [Vec<(Ring, usize)>; 2],
    /// room, hall and plate
    fdn: Box<crate::plugins2::Fdn>,
    pre: crate::plugins2::Line,
    cut: [[crate::kit::Biquad; 2]; 2],
    cut_at: (f32, f32),
    sr: f32,
}

impl Reverb {
    const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const ALLPASS: [usize; 4] = [556, 441, 341, 225];
    const SPREAD: usize = 23;

    fn new(sr: f32) -> Self {
        let scale = sr / 44100.0;
        let side = |extra: usize| {
            let combs = Self::COMBS.iter().map(|&c| {
                let d = ((c + extra) as f32 * scale) as usize;
                (Ring::new(d + 1), d, 0.0)
            }).collect();
            let aps = Self::ALLPASS.iter().map(|&a| {
                let d = ((a + extra) as f32 * scale) as usize;
                (Ring::new(d + 1), d)
            }).collect();
            (combs, aps)
        };
        let (cl, al) = side(0);
        let (cr, ar) = side(Self::SPREAD);
        Reverb { combs: [cl, cr], aps: [al, ar], fdn: Box::new(crate::plugins2::Fdn::new(sr)), pre: crate::plugins2::Line::new((sr * 0.21) as usize),
            cut: [[crate::kit::Biquad::new(); 2]; 2], cut_at: (-1.0, -1.0), sr }
    }
}

impl Effect for Reverb {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let kind = crate::kit::choice(p[5], 4);
        let predelay = 0.2 * p[6];
        let (lc_on, hc_on) = (p[7] > 0.005, p[8] < 0.995);
        let (lcf, hcf) = (20.0 * 50f32.powf(p[7]), 1000.0 * 20f32.powf(p[8]));
        if kind > 0 {
            self.fdn.process(kind - 1, p[0], p[1], p[3], p[2], p[4], predelay, if lc_on { lcf } else { 0.0 }, if hc_on { hcf } else { 0.0 }, p[9], l, r);
            return;
        }
        if (lcf, hcf) != self.cut_at {
            self.cut_at = (lcf, hcf);
            for side in &mut self.cut {
                side[0].c = crate::kit::rbj::highpass(lcf as f64, 0.7071, self.sr as f64);
                side[1].c = crate::kit::rbj::lowpass(hcf as f64, 0.7071, self.sr as f64);
            }
        }
        let pre = predelay * self.sr;
        let fb = 0.7 + 0.28 * p[0];
        let damp = p[1];
        let m = p[2];
        let w = p[3];
        let dry = (1.0 - 0.5 * m) * p[4];
        let (wet1, wet2) = (3.0 * m * (0.5 + 0.5 * w), 3.0 * m * (0.5 - 0.5 * w));
        for k in 0..l.len() {
            let mono = (l[k] + r[k]) * 0.5;
            self.pre.push(mono);
            let inp = if pre >= 1.0 { self.pre.read(pre) } else { mono } * 0.015;
            let mut out = [0.0f32; 2];
            for side in 0..2 {
                let mut o = 0.0;
                for (ring, d, store) in self.combs[side].iter_mut() {
                    let y = ring.tap(*d);
                    *store = y * (1.0 - damp) + *store * damp;
                    ring.write(inp + *store * fb);
                    o += y;
                }
                for (ring, d) in self.aps[side].iter_mut() {
                    let b = ring.tap(*d);
                    ring.write(o + b * 0.5);
                    o = b - o;
                }
                if lc_on {
                    o = self.cut[side][0].run(o);
                }
                if hc_on {
                    o = self.cut[side][1].run(o);
                }
                out[side] = o;
            }
            l[k] = l[k] * dry + out[0] * wet1 + out[1] * wet2;
            r[k] = r[k] * dry + out[1] * wet1 + out[0] * wet2;
        }
    }
    fn reset(&mut self) {
        self.fdn.reset();
        self.pre.clear();
        for side in 0..2 {
            for (r, _, s) in self.combs[side].iter_mut() {
                r.clear();
                *s = 0.0;
            }
            for (r, _) in self.aps[side].iter_mut() {
                r.clear();
            }
        }
    }
}

// --- EQ --------------------------------------------------------------------------

/// RBJ cookbook biquad coefficients, normalised: (b0, b1, b2, a1, a2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefs(pub [f32; 5]);

impl Coefs {
    pub const FLAT: Coefs = Coefs([1.0, 0.0, 0.0, 0.0, 0.0]);

    fn norm(b: [f64; 3], a: [f64; 3]) -> Coefs {
        Coefs([(b[0] / a[0]) as f32, (b[1] / a[0]) as f32, (b[2] / a[0]) as f32, (a[1] / a[0]) as f32, (a[2] / a[0]) as f32])
    }
    pub fn highpass(f: f64, q: f64, sr: f64) -> Coefs {
        let (s, c) = (2.0 * std::f64::consts::PI * f / sr).sin_cos();
        let al = s / (2.0 * q);
        Self::norm([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], [1.0 + al, -2.0 * c, 1.0 - al])
    }
    pub fn lowpass(f: f64, q: f64, sr: f64) -> Coefs {
        let (s, c) = (2.0 * std::f64::consts::PI * f / sr).sin_cos();
        let al = s / (2.0 * q);
        Self::norm([(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0], [1.0 + al, -2.0 * c, 1.0 - al])
    }
    pub fn peak(f: f64, q: f64, db: f64, sr: f64) -> Coefs {
        let a = 10f64.powf(db / 40.0);
        let (s, c) = (2.0 * std::f64::consts::PI * f / sr).sin_cos();
        let al = s / (2.0 * q);
        Self::norm([1.0 + al * a, -2.0 * c, 1.0 - al * a], [1.0 + al / a, -2.0 * c, 1.0 - al / a])
    }
    /// Shelves with slope 1.
    pub fn shelf(high: bool, f: f64, db: f64, sr: f64) -> Coefs {
        let a = 10f64.powf(db / 40.0);
        let (s, c) = (2.0 * std::f64::consts::PI * f / sr).sin_cos();
        let al = s / 2.0 * 2f64.sqrt();
        let sq = 2.0 * a.sqrt() * al;
        if high {
            Self::norm(
                [a * ((a + 1.0) + (a - 1.0) * c + sq), -2.0 * a * ((a - 1.0) + (a + 1.0) * c), a * ((a + 1.0) + (a - 1.0) * c - sq)],
                [(a + 1.0) - (a - 1.0) * c + sq, 2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - sq],
            )
        } else {
            Self::norm(
                [a * ((a + 1.0) - (a - 1.0) * c + sq), 2.0 * a * ((a - 1.0) - (a + 1.0) * c), a * ((a + 1.0) - (a - 1.0) * c - sq)],
                [(a + 1.0) + (a - 1.0) * c + sq, -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - sq],
            )
        }
    }
    /// Gain in dB at frequency f (the UI draws the same curve).
    pub fn db_at(&self, f: f64, sr: f64) -> f64 {
        let [b0, b1, b2, a1, a2] = self.0.map(|v| v as f64);
        let w = 2.0 * std::f64::consts::PI * f / sr;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let num = (b0 + b1 * c1 + b2 * c2).powi(2) + (b1 * s1 + b2 * s2).powi(2);
        let den = (1.0 + a1 * c1 + a2 * c2).powi(2) + (a1 * s1 + a2 * s2).powi(2);
        10.0 * (num / den).log10()
    }
}

/// The EQ's knobs in real units.
pub struct EqBands {
    pub lowcut: Option<f64>,
    pub low: (f64, f64), // Hz, dB
    pub mid: (f64, f64, f64), // Hz, dB, Q
    pub high: (f64, f64),
    pub highcut: Option<f64>,
}

pub fn eq_bands(p: &Params) -> EqBands {
    let gain = |v: f32| -15.0 + 30.0 * v as f64;
    EqBands {
        lowcut: (p[0] > 0.005).then(|| 20.0 * 40f64.powf(p[0] as f64)), // 20 Hz .. 800 Hz
        low: (30.0 * 20f64.powf(p[2] as f64), gain(p[1])), // 30 .. 600 Hz
        mid: (100.0 * 100f64.powf(p[4] as f64), gain(p[3]), 0.3 * 33.3f64.powf(p[5] as f64)), // 100 Hz .. 10 kHz, Q 0.3 .. 10
        high: (1500.0 * 10f64.powf(p[7] as f64), gain(p[6])), // 1.5 .. 15 kHz
        highcut: (p[8] < 0.995).then(|| 1000.0 * 20f64.powf(p[8] as f64)), // 1 .. 20 kHz
    }
}

/// The EQ's filters for these knobs: low cut (24 dB/octave: two stages),
/// low shelf, mid bell, high shelf, high cut (12 dB/octave).
pub fn eq_coefs(p: &Params, sr: f64) -> [Coefs; 6] {
    let b = eq_bands(p);
    let ny = sr * 0.45;
    let (hp1, hp2) = match b.lowcut {
        Some(f) => (Coefs::highpass(f.min(ny), 0.541, sr), Coefs::highpass(f.min(ny), 1.307, sr)),
        None => (Coefs::FLAT, Coefs::FLAT),
    };
    [
        hp1,
        hp2,
        if b.low.1.abs() < 0.05 { Coefs::FLAT } else { Coefs::shelf(false, b.low.0.min(ny), b.low.1, sr) },
        if b.mid.1.abs() < 0.05 { Coefs::FLAT } else { Coefs::peak(b.mid.0.min(ny), b.mid.2, b.mid.1, sr) },
        if b.high.1.abs() < 0.05 { Coefs::FLAT } else { Coefs::shelf(true, b.high.0.min(ny), b.high.1, sr) },
        match b.highcut {
            Some(f) => Coefs::lowpass(f.min(ny), std::f64::consts::FRAC_1_SQRT_2, sr),
            None => Coefs::FLAT,
        },
    ]
}

struct Eq {
    sr: f32,
    last: Option<Params>,
    c: [Coefs; 6],
    z: [[[f32; 2]; 6]; 2], // [side][stage][state]
}

impl Eq {
    fn new(sr: f32) -> Self {
        Eq { sr, last: None, c: [Coefs::FLAT; 6], z: [[[0.0; 2]; 6]; 2] }
    }
}

impl Effect for Eq {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        if self.last.as_ref() != Some(p) {
            self.c = eq_coefs(p, self.sr as f64);
            self.last = Some(*p);
        }
        for (side, buf) in [l, r].into_iter().enumerate() {
            for (stage, c) in self.c.iter().enumerate() {
                if *c == Coefs::FLAT {
                    continue;
                }
                let [b0, b1, b2, a1, a2] = c.0;
                let z = &mut self.z[side][stage];
                for x in buf.iter_mut() {
                    // transposed direct form II
                    let y = b0 * *x + z[0];
                    z[0] = b1 * *x - a1 * y + z[1];
                    z[1] = b2 * *x - a2 * y;
                    *x = y;
                }
            }
        }
    }
    fn reset(&mut self) {
        self.z = [[[0.0; 2]; 6]; 2];
    }
}

// --- limiter -----------------------------------------------------------------------

/// A look-ahead brickwall limiter: it sees a peak coming (the sound is delayed
/// by the look-ahead) and turns down smoothly before it, so nothing goes over
/// the ceiling. That delay is its latency: the song is read that much earlier.
pub struct Limiter {
    sr: f32,
    la: usize,
    ring: [Ring; 2],
    // the smallest gain needed over the look-ahead window, per side (monotonic queues)
    q: [MinQueue; 2],
    n: u64,
    env: [f32; 2],
    tp: crate::kit::TruePeak,
}

/// The smallest value pushed in the last `len` pushes.
struct MinQueue {
    val: Vec<f32>,
    at: Vec<u64>,
    head: usize,
    len: usize,
}

impl MinQueue {
    fn new(cap: usize) -> Self {
        MinQueue { val: vec![0.0; cap], at: vec![0; cap], head: 0, len: 0 }
    }
    fn push(&mut self, v: f32, n: u64, window: usize) {
        let cap = self.val.len();
        while self.len > 0 && self.val[(self.head + self.len - 1) % cap] >= v {
            self.len -= 1;
        }
        let i = (self.head + self.len) % cap;
        self.val[i] = v;
        self.at[i] = n;
        self.len += 1;
        while self.at[self.head] + (window as u64) < n {
            self.head = (self.head + 1) % cap;
            self.len -= 1;
        }
    }
    fn min(&self) -> f32 {
        self.val[self.head]
    }
}

impl Limiter {
    /// 1.5 ms
    pub fn lookahead(sr: f32) -> usize {
        ((sr * 0.0015).round() as usize).max(1)
    }

    fn new(sr: f32) -> Self {
        let la = Self::lookahead(sr);
        Limiter {
            sr, la, ring: [Ring::new(la + 1), Ring::new(la + 1)],
            q: [MinQueue::new(la + 2), MinQueue::new(la + 2)], n: 0, env: [1.0; 2], tp: crate::kit::TruePeak::new(),
        }
    }
}

impl Effect for Limiter {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let gin = db_to_lin(24.0 * p[0]);
        let ceil = db_to_lin(-12.0 + 12.0 * p[1]);
        let rel_s = 0.01 * 100f32.powf(p[2]);
        let link = p[3];
        let true_peak = p[4] >= 0.5;
        let rel = 1.0 - (-1.0 / (rel_s * self.sr)).exp();
        let att = 1.0 - (-3.0 / self.la as f32).exp(); // there in about the look-ahead
        for k in 0..l.len() {
            let (xl, xr) = (l[k] * gin, r[k] * gin);
            let (pl, pr) = if true_peak { (self.tp.peak(0, xl), self.tp.peak(1, xr)) } else { (xl.abs(), xr.abs()) };
            let both = pl.max(pr);
            for (side, pk) in [(0, pl), (1, pr)] {
                let pk = pk + (both - pk) * link;
                self.q[side].push(if pk > ceil { ceil / pk } else { 1.0 }, self.n, self.la);
                let want = self.q[side].min();
                self.env[side] += (want - self.env[side]) * if want < self.env[side] { att } else { rel };
            }
            self.n += 1;
            self.ring[0].write(xl);
            self.ring[1].write(xr);
            // the hard limit catches what the smoothing didn't quite reach
            l[k] = (self.ring[0].tap(self.la + 1) * self.env[0]).clamp(-ceil, ceil);
            r[k] = (self.ring[1].tap(self.la + 1) * self.env[1]).clamp(-ceil, ceil);
        }
    }
    fn reset(&mut self) {
        for r in self.ring.iter_mut() {
            r.clear();
        }
        for q in self.q.iter_mut() {
            q.len = 0;
        }
        self.env = [1.0; 2];
    }
    fn gain_reduction(&self) -> f32 {
        -20.0 * self.env[0].min(self.env[1]).max(1e-6).log10()
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
            r.insert(i, Slot::new(c.clone(), 48000.0).unwrap()).ok().unwrap();
        }
        r
    }

    /// A mono signal through a rack in blocks: (left, right).
    fn run(r: &mut Rack, x: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut rr) = (x.to_vec(), x.to_vec());
        for (a, b) in l.chunks_mut(block).zip(rr.chunks_mut(block)) {
            r.process(a, b, &no_keys);
        }
        (l, rr)
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |a, v| a.max(v.abs()))
    }

    #[test]
    fn kinds_are_where_their_constants_say() {
        for (k, name) in [(COMP, "comp"), (NAM, "nam"), (VST, "vst"), (EQ, "eq"), (LIMITER, "limiter"), (EQPRO, "eqpro"), (CLIPPER, "clipper"),
            (MULTI, "multi"), (SATURATE, "saturate"), (SHAPER, "shaper"), (DEESS, "deess"), (TRANSIENT, "transient"), (WIDTH, "width"),
            (GATE, "gate"), (FILTER, "filter"), (LOFI, "lofi"), (UTILITY, "utility"), (DOUBLER, "doubler"), (AUTOTUNE, "autotune")] {
            assert_eq!(FX[k].name, name);
        }
        for f in FX.iter() {
            for st in f.stepped {
                assert!(f.params.contains(st), "{}: {st}", f.name);
            }
        }
    }

    #[test]
    fn every_effect_is_finite_bounded_and_audible() {
        for kind in (0..N_KINDS).filter(|&k| k != NAM && k != VST) { // NAM and VST need a file; tested on their own
            for block in [64usize, 128, 512] {
                let mut r = rack_of(&[SlotCfg::new(kind)]);
                let (l, rr) = run(&mut r, &noise(96000), block);
                for x in [&l, &rr] {
                    assert!(x.iter().all(|v| v.is_finite()), "{}", FX[kind].name);
                    assert!(peak(x) < 4.0, "{} peak {}", FX[kind].name, peak(x));
                    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
                    assert!(rms > 0.01, "{}", FX[kind].name);
                }
            }
        }
    }

    #[test]
    fn delay_echo_lands_on_time_and_ping_pong_bounces() {
        let mut s = SlotCfg::new(4);
        s.p[..3].copy_from_slice(&[0.0, 0.0, 0.35]); // 60 ms, no repeats
        let mut x = vec![0f32; 48000];
        x[0] = 1.0;
        let (l, _) = run(&mut rack_of(&[s.clone()]), &x, 128);
        let echo = (100..l.len()).max_by(|&a, &b| l[a].abs().total_cmp(&l[b].abs())).unwrap();
        assert!((echo as i64 - 2880).abs() <= 1, "echo at {}", echo);
        // ping-pong: the first echo on the left, the next one on the right
        s.p[1] = 0.6;
        s.p[3] = 1.0;
        let (l, r) = run(&mut rack_of(&[s]), &x, 128);
        assert!(l[2880].abs() > 0.05 && r[2880].abs() < 1e-3, "first echo left");
        assert!(r[5760].abs() > 0.02 && l[5760].abs() < 1e-3, "second echo right");
    }

    #[test]
    fn reverb_decays_and_is_wide() {
        let mut s = SlotCfg::new(5);
        s.p[..3].copy_from_slice(&[1.0, 0.0, 0.3]);
        let mut x = vec![0f32; 48000 * 8];
        x[..128].fill(1.0);
        let (l, r) = run(&mut rack_of(&[s]), &x, 128);
        assert!(peak(&l[l.len() - 48000..]) < peak(&l[..48000]) * 0.1);
        // a mono input comes out different on each side
        let diff: f32 = l[4800..48000].iter().zip(&r[4800..48000]).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 1.0, "left and right tails differ ({diff})");
    }

    #[test]
    fn eq_curve_does_what_the_knobs_say() {
        let sr = 48000.0;
        let gain = |p: &Params, f: f64| eq_coefs(p, sr).iter().map(|c| c.db_at(f, sr)).sum::<f64>();
        let flat = FX[EQ].defaults;
        for f in [30.0, 200.0, 1000.0, 5000.0, 15000.0] {
            assert!(gain(&flat, f).abs() < 0.01, "flat at {f} Hz: {}", gain(&flat, f));
        }
        let mut p = flat;
        p[0] = 0.5; // low cut at ~126 Hz
        assert!(gain(&p, 30.0) < -40.0 && gain(&p, 1000.0).abs() < 0.5, "low cut");
        let mut p = flat;
        p[3] = 0.7; // +6 dB at 1 kHz
        assert!((gain(&p, 1000.0) - 6.0).abs() < 0.05 && gain(&p, 50.0).abs() < 0.5, "mid bell");
        let mut p = flat;
        p[6] = 0.3; // -6 dB high shelf
        assert!((gain(&p, 18000.0) + 6.0).abs() < 0.6 && gain(&p, 100.0).abs() < 0.3, "high shelf");
        // and the filters really do it: a 50 Hz tone through the low cut
        let mut s = SlotCfg::new(EQ);
        s.p[0] = 0.5;
        let x: Vec<f32> = (0..48000).map(|i| (2.0 * PI * 50.0 * i as f32 / 48000.0).sin() * 0.5).collect();
        let (l, r) = run(&mut rack_of(&[s]), &x, 128);
        assert!(peak(&l[24000..]) < 0.02 && peak(&r[24000..]) < 0.02, "{}", peak(&l[24000..]));
    }

    #[test]
    fn limiter_true_peak_catches_what_falls_between_the_samples() {
        // a sine at a quarter of the rate, phased so every sample is at 0.707 of its real peak
        let x: Vec<f32> = (0..48000).map(|i| (PI / 2.0 * i as f32 + PI / 4.0).sin()).collect();
        let mut s = SlotCfg::new(LIMITER);
        s.p[1] = 0.5; // -6 dB
        let ceil = db_to_lin(-6.0);
        let (l, _) = run(&mut rack_of(&[s.clone()]), &x, 128);
        let mut tp = crate::kit::TruePeak::new();
        let real = l[24000..].iter().map(|v| tp.peak(0, *v)).fold(0f32, f32::max);
        assert!(real <= ceil * 1.02, "true peak {real} over {ceil}");
        s.p[4] = 0.0;
        let (l, _) = run(&mut rack_of(&[s]), &x, 128);
        let mut tp = crate::kit::TruePeak::new();
        let real = l[24000..].iter().map(|v| tp.peak(0, *v)).fold(0f32, f32::max);
        assert!(real > ceil * 1.2, "sample peaks only: {real}");
    }

    #[test]
    fn limiter_holds_the_ceiling_and_reports_its_delay() {
        let mut s = SlotCfg::new(LIMITER);
        s.p[0] = 0.5; // +12 dB in
        s.p[1] = 0.5; // ceiling -6 dB
        s.p[4] = 0.0; // sample peaks (white noise really does go higher between its samples)
        let ceil = db_to_lin(-6.0);
        assert_eq!(s.latency(48000.0), Limiter::lookahead(48000.0) as u32);
        let x = noise(48000);
        let (l, r) = run(&mut rack_of(&[s.clone()]), &x, 128);
        assert!(peak(&l) <= ceil + 1e-6 && peak(&r) <= ceil + 1e-6, "{}", peak(&l));
        assert!(peak(&l) > ceil * 0.9, "and loud {} vs {}", peak(&l), ceil);
        // an impulse comes out exactly the look-ahead later
        s.p[0] = 0.0;
        s.p[1] = 1.0;
        let mut x = vec![0f32; 1000];
        x[10] = 0.5;
        let (l, _) = run(&mut rack_of(&[s.clone()]), &x, 128);
        let at = (0..l.len()).max_by(|&a, &b| l[a].abs().total_cmp(&l[b].abs())).unwrap();
        assert_eq!(at, 10 + Limiter::lookahead(48000.0));
        s.on = false;
        assert_eq!(s.latency(48000.0), 0, "off: no delay");
    }

    #[test]
    fn a_keyed_compressor_ducks_to_the_other_track() {
        let mut s = SlotCfg::new(COMP);
        s.p[0] = 0.2; // -33 dB
        s.p[1] = 1.0; // 12:1
        s.p[2] = 0.0;
        s.key = Some(3);
        let tone: Vec<f32> = (0..9600).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let loud = vec![0.8f32; 9600];
        let quiet = vec![0f32; 9600];
        let out = |key: &Vec<f32>| {
            let mut r = rack_of(&[s.clone()]);
            let (mut l, mut rr) = (tone.clone(), tone.clone());
            for k in 0..(9600 / 128) {
                let range = k * 128..(k + 1) * 128;
                let keys = |slot: usize| (slot == 3).then(|| (&key[range.clone()], &key[range.clone()]));
                r.process(&mut l[range.clone()], &mut rr[range.clone()], &keys);
            }
            peak(&l[4800..])
        };
        let ducked = out(&loud);
        let free = out(&quiet);
        assert!(free > 0.25 && ducked < free * 0.2, "free {free}, ducked {ducked}");
    }

    #[test]
    fn rack_order_matters_and_never_grows_past_capacity() {
        // drive then reverb != reverb then drive
        let (d, v) = (SlotCfg::new(1), SlotCfg::new(5));
        let (xa, _) = run(&mut rack_of(&[d.clone(), v.clone()]), &noise(4800), 4800);
        let mut b = rack_of(&[v, d]);
        let (xb, _) = run(&mut b, &noise(4800), 4800);
        assert!(xa.iter().zip(&xb).any(|(p, q)| (p - q).abs() > 1e-3));
        // move puts it back in the same order
        b.move_slot(1, 0);
        assert_eq!(b.slots[0].cfg.kind, 1);
        let mut full = Rack::new();
        let cap = full.slots.capacity();
        for i in 0..MAX_SLOTS {
            assert!(full.insert(i, Slot::new(SlotCfg::new(0), 48000.0).unwrap()).is_ok());
        }
        assert!(full.insert(0, Slot::new(SlotCfg::new(0), 48000.0).unwrap()).is_err());
        assert_eq!(full.slots.capacity(), cap, "rack reallocated");
    }

    const TINY_NAM: &str = r#"{
        "version": "0.5.4", "architecture": "WaveNet",
        "metadata": {"name": "Tiny Test Amp", "loudness": -24.0},
        "config": {
            "layers": [{
                "input_size": 1, "condition_size": 1, "channels": 1, "head_size": 1,
                "kernel_size": 1, "dilations": [1], "activation": "ReLU",
                "gated": false, "head_bias": false
            }],
            "head": null, "head_scale": 1.0
        },
        "weights": [1.0, 2.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0]
    }"#;

    #[test]
    fn nam_slot_loads_a_model_and_processes() {
        let path = std::env::temp_dir().join("duet_tiny_test.nam");
        std::fs::write(&path, TINY_NAM).unwrap();
        let mut cfg = SlotCfg::new(NAM);
        cfg.model = Some(path.to_string_lossy().into_owned());
        let slot = Slot::new(cfg, 48000.0).unwrap();
        let info = slot.cfg.info.clone().unwrap();
        assert_eq!(info.name, "Tiny Test Amp");
        assert_eq!(info.sample_rate, 48000.0);
        assert_eq!(info.loudness, Some(-24.0));
        let mut r = Rack::new();
        r.insert(0, slot).ok().unwrap();
        let before = noise(4800);
        let (l, rr) = run(&mut r, &before, 128);
        assert!(l.iter().all(|v| v.is_finite()));
        assert!(l.iter().zip(&before).any(|(a, b)| (a - b).abs() > 1e-3), "model didn't change the sound");
        assert_eq!(l, rr, "an amp is mono: the same on both sides");
        // empty NAM slot (no model yet) passes audio through untouched
        let (y, _) = run(&mut rack_of(&[SlotCfg::new(NAM)]), &before, 4800);
        assert_eq!(y, before);
        // a missing file is an error, not a crash
        let mut bad = SlotCfg::new(NAM);
        bad.model = Some("Z:/nope/missing.nam".into());
        assert!(Slot::new(bad, 48000.0).is_err());
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
