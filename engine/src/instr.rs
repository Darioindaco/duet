//! Instruments: what a strip plays instead of an interface input. Notes come
//! from a MIDI keyboard, the computer keyboard or (later) the song.
//!
//! Same realtime rule as the effects: built (and, for the drum kit, rendered)
//! on the control side, handed to the audio thread in a Box, handed back to be
//! freed. Output is mono, like every strip.

use crate::fx::VstRef;

pub const DRUMS: usize = 0;
pub const SYNTH: usize = 1;
pub const VSTI: usize = 2;
/// one sample, played at the pitch of each note (FL's sampler channel); the
/// sample is `pads[0]`
pub const SAMPLER: usize = 3;
pub const KINDS: [&str; 4] = ["drums", "synth", "vst", "sampler"];
pub const N_PARAMS: usize = 96;

/// The drum kit's knobs: the kit's own, then five per pad (see drums.rs).
pub const DRUM_PARAMS: [&str; 85] = [
    "level", "tune", "decay", "kit", "drive", "p0_tune", "p0_decay", "p0_tone",
    "p0_level", "p0_pan", "p1_tune", "p1_decay", "p1_tone", "p1_level", "p1_pan", "p2_tune",
    "p2_decay", "p2_tone", "p2_level", "p2_pan", "p3_tune", "p3_decay", "p3_tone", "p3_level",
    "p3_pan", "p4_tune", "p4_decay", "p4_tone", "p4_level", "p4_pan", "p5_tune", "p5_decay",
    "p5_tone", "p5_level", "p5_pan", "p6_tune", "p6_decay", "p6_tone", "p6_level", "p6_pan",
    "p7_tune", "p7_decay", "p7_tone", "p7_level", "p7_pan", "p8_tune", "p8_decay", "p8_tone",
    "p8_level", "p8_pan", "p9_tune", "p9_decay", "p9_tone", "p9_level", "p9_pan", "p10_tune",
    "p10_decay", "p10_tone", "p10_level", "p10_pan", "p11_tune", "p11_decay", "p11_tone", "p11_level",
    "p11_pan", "p12_tune", "p12_decay", "p12_tone", "p12_level", "p12_pan", "p13_tune", "p13_decay",
    "p13_tone", "p13_level", "p13_pan", "p14_tune", "p14_decay", "p14_tone", "p14_level", "p14_pan",
    "p15_tune", "p15_decay", "p15_tone", "p15_level", "p15_pan",
];

/// Knobs per kind (0..1), and where they start. By name in the song, so a
/// kind can gain knobs and old songs still load.
pub const PARAMS: [&[&str]; 4] = [
    &DRUM_PARAMS,
    &crate::synth::NAMES,
    &[],
    // root: the key that plays the sample as it is (0.5 = C4); oneshot: play to
    // the end whatever the note's length (drum hits)
    &["level", "tune", "attack", "release", "root", "oneshot"],
];
pub const DEFAULTS: [[f32; N_PARAMS]; 4] = [crate::drums::defaults(), crate::synth::defaults(), [0.0; N_PARAMS], sampler_defaults()];

const fn sampler_defaults() -> [f32; N_PARAMS] {
    let mut p = [0f32; N_PARAMS];
    p[0] = 0.8;
    p[1] = 0.5;
    p[3] = 0.15;
    p[4] = 0.5;
    p
}

/// A built-in drum sound, as a sample (the Browser's "Duet sounds").
pub fn drum_sound(i: usize, sr: f32) -> Vec<f32> {
    crate::drums::render_pad(i, sr)
}

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
    /// Write one stereo block (replaces what's in `l` and `r`).
    fn process(&mut self, p: &[f32; N_PARAMS], l: &mut [f32], r: &mut [f32]);
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
                let (kit, err) = crate::drums::DrumKit::new(sr, &cfg.pads);
                cfg.error = err;
                Box::new(kit)
            }
            SYNTH => Box::new(crate::synth::Synth::new(sr)),
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
            SAMPLER => {
                let (smp, err) = Sampler::new(sr, cfg.pads.first().cloned().flatten());
                cfg.error = err;
                Box::new(smp)
            }
            k => return Err(format!("no instrument kind {k}")),
        };
        Ok(Box::new(InstSlot { cfg, inst }))
    }
}

// --- the sampler --------------------------------------------------------------------

struct SmpVoice {
    key: u8,
    pos: f64,
    step: f64,
    gain: f32,
    env: f32,
    on: bool,
    held: bool,
}

/// One sample (mono or stereo) played at each note's pitch: the root key
/// plays it as it is, an octave up twice as fast. 16 voices.
pub struct Sampler {
    l: Vec<f32>,
    r: Option<Vec<f32>>,
    voices: Vec<SmpVoice>,
    sr: f32,
    sustain: bool,
}

const SAMPLER_VOICES: usize = 16;

impl Sampler {
    pub fn new(sr: f32, path: Option<String>) -> (Sampler, Option<String>) {
        let mut s = Sampler { l: vec![], r: None, voices: Vec::with_capacity(SAMPLER_VOICES), sr, sustain: false };
        let Some(path) = path else { return (s, None) };
        match crate::decode::read_audio(std::path::Path::new(&path)) {
            Ok(a) => {
                s.l = crate::daw::resample(&a.samples, a.sr, sr as u32);
                s.r = a.right.as_ref().map(|r| crate::daw::resample(r, a.sr, sr as u32));
                (s, None)
            }
            Err(e) => (s, Some(e)),
        }
    }
}

impl Instrument for Sampler {
    fn note(&mut self, ev: NoteEv) {
        match ev {
            NoteEv::On(key, vel) => {
                if self.voices.len() >= SAMPLER_VOICES {
                    self.voices.remove(0); // (capacity is reserved: no allocation)
                }
                let g = vel as f32 / 127.0;
                self.voices.push(SmpVoice { key, pos: 0.0, step: 0.0, gain: g * g, env: 0.0, on: true, held: false });
            }
            NoteEv::Off(key) => {
                for v in self.voices.iter_mut().filter(|v| v.key == key && v.on) {
                    if self.sustain { v.held = true } else { v.on = false }
                }
            }
            NoteEv::Sustain(down) => {
                self.sustain = down;
                if !down {
                    for v in self.voices.iter_mut().filter(|v| v.held) {
                        v.held = false;
                        v.on = false;
                    }
                }
            }
            NoteEv::Release => {
                self.sustain = false;
                for v in self.voices.iter_mut() {
                    v.on = false;
                }
            }
            NoteEv::AllOff => self.voices.clear(),
        }
    }

    fn process(&mut self, p: &[f32; N_PARAMS], out_l: &mut [f32], out_r: &mut [f32]) {
        out_l.fill(0.0);
        out_r.fill(0.0);
        if self.l.len() < 2 {
            self.voices.clear();
            return;
        }
        let level = p[0] * p[0] * 1.6;
        let tune = (p[1] - 0.5) * 24.0; // ±12 semitones
        let attack = 0.0005 + 0.5 * p[2] * p[2]; // up to 0.5 s
        let release = 0.005 + 3.0 * p[3] * p[3]; // up to 3 s
        let root = 60.0 + ((p[4] - 0.5) * 72.0).round(); // C1 .. C7
        let oneshot = p[5] >= 0.5;
        let a_step = 1.0 / (attack * self.sr);
        let r_mul = (-1.0 / (release * self.sr)).exp();
        let n = self.l.len();
        let right = self.r.as_ref().unwrap_or(&self.l);
        for v in self.voices.iter_mut() {
            v.step = 2f64.powf((v.key as f64 - root as f64 + tune as f64) / 12.0);
            for (ol, or) in out_l.iter_mut().zip(out_r.iter_mut()) {
                let i = v.pos as usize;
                if i + 1 >= n {
                    v.env = 0.0;
                    break;
                }
                let t = (v.pos - i as f64) as f32;
                let xl = self.l[i] + (self.l[i + 1] - self.l[i]) * t;
                let xr = right[i] + (right[i + 1] - right[i]) * t;
                let playing = v.on || oneshot;
                v.env = if playing { (v.env + a_step).min(1.0) } else { v.env * r_mul };
                let g = v.gain * level * v.env;
                *ol += xl * g;
                *or += xr * g;
                v.pos += v.step;
            }
        }
        self.voices.retain(|v| v.env > 1e-4 || (v.on && v.pos < 1.0));
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

    fn process(&mut self, _p: &[f32; N_PARAMS], l: &mut [f32], r: &mut [f32]) {
        for i in 0..self.n_pending {
            if let Some((on, k, v)) = self.pending[i].take() {
                self.proc.note(on, k, v, 0);
            }
        }
        self.n_pending = 0;
        l.fill(0.0);
        r.fill(0.0);
        let tempo = f64::from_bits(crate::fx::TEMPO_BITS.load(std::sync::atomic::Ordering::Relaxed));
        self.proc.process(l, r, 1.0, tempo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drums::DrumKit;
    use crate::synth::Synth;
    use std::f32::consts::PI;

    fn run(inst: &mut dyn Instrument, p: &[f32; N_PARAMS], blocks: usize) -> Vec<f32> {
        run2(inst, p, blocks).0
    }
    fn run2(inst: &mut dyn Instrument, p: &[f32; N_PARAMS], blocks: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (Vec::new(), Vec::new());
        let (mut bl, mut br) = ([0f32; 128], [0f32; 128]);
        for _ in 0..blocks {
            inst.process(p, &mut bl, &mut br);
            l.extend_from_slice(&bl);
            r.extend_from_slice(&br);
        }
        (l, r)
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

    #[test]
    fn the_sampler_plays_its_sample_at_the_notes_pitch() {
        let dir = std::env::temp_dir().join(format!("duet-smp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a440.wav");
        let spec = hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..48000 {
            w.write_sample(((2.0 * PI * 440.0 * i as f32 / 48000.0).sin() * 16000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        let (mut s, err) = Sampler::new(48000.0, Some(path.to_string_lossy().into_owned()));
        assert!(err.is_none());
        let p = DEFAULTS[SAMPLER];
        let pitch = |s: &mut Sampler, key: u8| {
            s.note(NoteEv::AllOff);
            s.note(NoteEv::On(key, 127));
            let x = run(s, &p, 40);
            let cross = x[2000..].windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
            cross as f32 / ((x.len() - 2000) as f32 / 48000.0)
        };
        assert!((pitch(&mut s, 60) - 440.0).abs() < 10.0, "the root plays it as it is");
        assert!((pitch(&mut s, 72) - 880.0).abs() < 20.0, "an octave up");
        // a note off lets it fade; a one-shot plays on
        s.note(NoteEv::Off(72));
        let gone = run(&mut s, &p, 200);
        assert!(peak(&gone[gone.len() - 512..]) < 1e-3);
        let (_, err) = Sampler::new(48000.0, Some("/nope/missing.wav".into()));
        assert!(err.is_some(), "a missing file says so");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_synth_is_wide_when_detuned_and_centred_when_not() {
        let mut p = DEFAULTS[SYNTH];
        for (detune, wide) in [(0.0, false), (1.0, true)] {
            p[5] = detune;
            let mut s = Synth::new(48000.0);
            s.note(NoteEv::On(57, 100));
            let (l, r) = run2(&mut s, &p, 200);
            let diff = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
            assert_eq!(diff > 0.01, wide, "detune {detune}: {diff}");
            assert!(peak(&r) > 0.05);
        }
    }
}
