// Duet — the plugin kit: what every built-in effect's window is made of.
//
// A window has a bar (presets with ◀ ▶, save, A/B with copy, undo/redo, reset), a display (a
// graph of what the effect does, drawn live: the EQ's curve with handles to drag, a
// compressor's transfer curve with the signal moving on it, a limiter's gain reduction over
// time, the echoes of a delay, a reverb's tail, an LFO), meters (in, out, gain reduction) and
// knobs. The knobs: drag (Shift: fine), scroll, double-click for the default, Alt/Ctrl-click
// to type a value, right-click to automate. Values map to the engine's 0..1 knobs exactly as
// fx.rs does (PK_SPEC).

"use strict";

// ---------- what each knob means ----------
// [key, label, kind, a, b, unit, extra]: lin a..b, log a..b, pct; "off" values at the ends
const EQP_TYPES = ["Bell", "Low shelf", "High shelf", "Low cut", "High cut", "Notch", "Band pass"];
const PK_SPEC = {
  eq: [["lowcut", "Low cut", "log", 20, 800, "Hz", { offBelow: 0.005 }], ["low", "Low", "lin", -15, 15, "dB"], ["lowfreq", "Low freq", "log", 30, 600, "Hz"],
    ["mid", "Mid", "lin", -15, 15, "dB"], ["midfreq", "Mid freq", "log", 100, 10000, "Hz"], ["q", "Q", "log", 0.3, 9.99, ""],
    ["high", "High", "lin", -15, 15, "dB"], ["highfreq", "High freq", "log", 1500, 15000, "Hz"], ["highcut", "High cut", "log", 1000, 20000, "Hz", { offAbove: 0.995 }]],
  comp: [["threshold", "Threshold", "lin", -40, -4, "dB"], ["ratio", "Ratio", "lin", 1, 12, ":1"], ["attack", "Attack", "log", 0.1, 100, "ms"],
    ["release", "Release", "log", 10, 1000, "ms"], ["makeup", "Makeup", "lin", 0, 18, "dB"], ["knee", "Knee", "lin", 0, 24, "dB"],
    ["mix", "Mix", "pct"], ["schp", "SC low cut", "log", 20, 500, "Hz", { offBelow: 0.005 }], ["char", "Character", "choice", ["Clean", "Glue", "Punch", "Smooth"]]],
  limiter: [["gain", "Input gain", "lin", 0, 24, "dB"], ["ceiling", "Ceiling", "lin", -12, 0, "dB"], ["release", "Release", "log", 10, 1000, "ms"],
    ["link", "Stereo link", "pct"], ["tp", "True peak", "toggle"]],
  clipper: [["input", "Input", "lin", 0, 24, "dB"], ["ceiling", "Ceiling", "lin", -24, 0, "dB"], ["knee", "Softness", "pct"],
    ["os", "Oversampling", "choice", ["Off", "2x", "4x", "8x"]], ["mix", "Mix", "pct"], ["output", "Output", "lin", -12, 12, "dB"], ["delta", "Delta", "toggle"]],
  saturate: [["drive", "Drive", "lin", 0, 36, "dB"], ["mode", "Mode", "choice", ["Tape", "Tube", "Transistor", "Fuzz", "Rectify"]], ["bias", "Asymmetry", "pct"],
    ["tone", "Tilt", "lin", -6, 6, "dB"], ["mix", "Mix", "pct"], ["output", "Output", "lin", -12, 12, "dB"],
    ["os", "Oversampling", "choice", ["Off", "2x", "4x", "8x"]], ["auto", "Auto gain", "toggle"]],
  multi: [["depth", "Depth", "pct"], ["time", "Time", "log", 0.1, 10, "x"], ["upward", "Upward", "pct"], ["downward", "Downward", "pct"],
    ["input", "Input", "lin", -12, 24, "dB"], ["output", "Output", "lin", -12, 12, "dB"], ["xlow", "Low / mid", "log", 40, 1000, "Hz"], ["xhigh", "Mid / high", "log", 1000, 12000, "Hz"],
    ...["low", "mid", "high"].flatMap((b) => [[b + "down", "Down", "lin", -60, 0, "dB"], [b + "up", "Up", "lin", -60, 0, "dB"], [b + "gain", "Gain", "lin", -24, 24, "dB"]])],
  shaper: [["depth", "Depth", "pct"], ["rate", "Rate", "choice", ["1/32", "1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars"]],
    ["shape", "Shape", "choice", ["Pump", "Sine", "Square", "Saw down", "Saw up", "Gate"]], ["curve", "Curve", "pct"], ["hold", "Hold", "lin", 0, 50, "%"],
    ["phase", "Phase", "lin", 0, 360, "°"], ["trig", "Trigger", "choice", ["Tempo", "Sidechain"]], ["thresh", "Threshold", "lin", -48, 0, "dB"],
    ["band", "Band", "choice", ["Full", "Lows", "Highs"]], ["xover", "Split", "log", 40, 2000, "Hz"], ["mix", "Mix", "pct"], ["smooth", "Smooth", "log", 0.5, 20, "ms"]],
  eqpro: [...Array.from({ length: 8 }, (_, i) => i + 1).flatMap((b) => [
    [`b${b}on`, "On", "toggle"], [`b${b}type`, "Type", "choice", EQP_TYPES], [`b${b}freq`, "Freq", "log", 20, 20000, "Hz"],
    [`b${b}gain`, "Gain", "lin", -24, 24, "dB"], [`b${b}q`, "Q", "log", 0.1, 18, ""], [`b${b}slope`, "Slope", "choice", ["12 dB", "24 dB", "48 dB"]],
    [`b${b}ch`, "Channel", "choice", ["Stereo", "Mid", "Side", "Left", "Right"]], [`b${b}dyn`, "Dynamic", "lin", -24, 24, "dB"], [`b${b}thr`, "Threshold", "lin", -60, 0, "dB"]]),
    ["output", "Output", "lin", -24, 24, "dB"], ["auto", "Auto gain", "toggle"]],
  drive: [["drive", "Drive", "pct"], ["tone", "Tone", "pct"], ["level", "Level", "pct"]],
  chorus: [["rate", "Rate", "lin", 0.1, 3.1, "Hz"], ["depth", "Depth", "pct"], ["mix", "Mix", "pct"]],
  tremolo: [["rate", "Rate", "lin", 0.5, 12, "Hz"], ["depth", "Depth", "pct"], ["stereo", "Stereo", "pct"]],
  delay: [["time", "Time", "lin", 60, 1160, "ms"], ["feedback", "Feedback", "pct"], ["mix", "Mix", "pct"], ["pingpong", "Ping-pong", "pct"], ["dry", "Dry", "pct"],
    ["sync", "Sync", "choice", ["Free", "1/32", "1/16T", "1/16", "1/16D", "1/8T", "1/8", "1/8D", "1/4T", "1/4", "1/4D", "1/2", "1 bar"]],
    ["lowcut", "Low cut", "log", 20, 2000, "Hz", { offBelow: 0.005 }], ["highcut", "High cut", "log", 1000, 20000, "Hz", { offAbove: 0.995 }],
    ["drive", "Drive", "pct"], ["wow", "Wow", "pct"], ["duck", "Duck", "pct"]],
  reverb: [["size", "Size", "pct"], ["damp", "Damping", "pct"], ["width", "Width", "pct"], ["mix", "Mix", "pct"], ["dry", "Dry", "pct"],
    ["type", "Type", "choice", ["Classic", "Room", "Hall", "Plate"]], ["predelay", "Pre-delay", "lin", 0, 200, "ms"],
    ["lowcut", "Low cut", "log", 20, 1000, "Hz", { offBelow: 0.005 }], ["highcut", "High cut", "log", 1000, 20000, "Hz", { offAbove: 0.995 }], ["mod", "Movement", "pct"]],
  deess: [["freq", "Frequency", "log", 2000, 12000, "Hz"], ["thresh", "Threshold", "lin", -60, 0, "dB"], ["range", "Range", "lin", 0, 24, "dB"],
    ["mode", "Mode", "choice", ["Split", "Wide"]], ["listen", "Listen", "toggle"]],
  transient: [["attack", "Attack", "lin", -100, 100, "%"], ["sustain", "Sustain", "lin", -100, 100, "%"], ["speed", "Length", "lin", 0.5, 2, "x"],
    ["output", "Output", "lin", -12, 12, "dB"], ["mix", "Mix", "pct"]],
  width: [["width", "Width", "lin", 0, 200, "%"], ["lowwidth", "Low width", "lin", 0, 200, "%"], ["highwidth", "High width", "lin", 0, 200, "%"],
    ["xover", "Split", "log", 100, 5000, "Hz"], ["monobass", "Mono below", "log", 20, 500, "Hz", { offBelow: 0.005 }], ["balance", "Balance", "lin", -100, 100, "%"],
    ["output", "Output", "lin", -12, 12, "dB"]],
  gate: [["thresh", "Threshold", "lin", -80, 0, "dB"], ["range", "Range", "lin", 0, 80, "dB"], ["attack", "Attack", "log", 0.05, 50, "ms"], ["hold", "Hold", "lin", 0, 500, "ms"],
    ["release", "Release", "log", 5, 2000, "ms"], ["hyst", "Hysteresis", "lin", 0, 12, "dB"], ["schp", "SC low cut", "log", 20, 2000, "Hz", { offBelow: 0.005 }],
    ["sclp", "SC high cut", "log", 500, 20000, "Hz", { offAbove: 0.995 }], ["listen", "Listen", "toggle"]],
  filter: [["type", "Type", "choice", ["Low pass", "High pass", "Band pass", "Notch"]], ["slope", "Slope", "choice", ["12 dB", "24 dB"]],
    ["cutoff", "Cutoff", "log", 20, 20000, "Hz"], ["reso", "Resonance", "pct"], ["drive", "Drive", "lin", 0, 24, "dB"],
    ["rate", "LFO rate", "choice", ["1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars", "4 bars"]], ["shape", "LFO shape", "choice", ["Sine", "Triangle", "Saw", "Square", "Random"]],
    ["lfo", "LFO depth", "lin", -4, 4, "oct"], ["env", "Envelope", "lin", -4, 4, "oct"], ["mix", "Mix", "pct"]],
  lofi: [["bits", "Bits", "lin", 2, 16, "bit"], ["rate", "Rate", "log", 500, 48000, "Hz"], ["wow", "Wow", "pct"], ["flutter", "Flutter", "pct"],
    ["noise", "Noise", "pct"], ["tone", "Tone", "log", 1000, 20000, "Hz"], ["mix", "Mix", "pct"], ["output", "Output", "lin", -12, 12, "dB"]],
  utility: [["gain", "Gain", "lin", -24, 24, "dB"], ["pan", "Pan", "lin", -100, 100, "%"], ["width", "Width", "lin", 0, 200, "%"], ["phasel", "Flip L", "toggle"],
    ["phaser", "Flip R", "toggle"], ["mono", "Mono", "toggle"], ["swap", "Swap L/R", "toggle"], ["dc", "DC cut", "toggle"]],
  autotune: [["key", "Key", "choice", ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"]],
    ["scale", "Scale", "choice", ["Chromatic", "Major", "Minor", "Harmonic minor", "Pentatonic major", "Pentatonic minor", "Blues"]],
    ["speed", "Retune speed", "sq", 0, 400, "ms"], ["humanize", "Humanize", "pct"], ["amount", "Amount", "pct"], ["transpose", "Transpose", "lin", -12, 12, "st"], ["mix", "Mix", "pct"]],
  doubler: [["detune", "Detune", "lin", 0, 50, "ct"], ["delay", "Delay", "lin", 0, 40, "ms"], ["width", "Width", "pct"], ["voices", "Voices", "choice", ["2", "4", "6", "8"]],
    ["mix", "Mix", "pct"], ["lowcut", "Low cut", "log", 20, 1000, "Hz", { offBelow: 0.005 }]],
  nam: [["input", "Input", "lin", -12, 12, "dB"], ["output", "Output", "lin", -12, 12, "dB"]],
  vst: [["mix", "Mix", "pct"]],
};
// which knobs sit together, and under which heading
const PK_GROUPS = {
  eq: [["Cuts", ["lowcut", "highcut"]], ["Low", ["low", "lowfreq"]], ["Mid", ["mid", "midfreq", "q"]], ["High", ["high", "highfreq"]]],
  comp: [["Detector", ["threshold", "ratio", "knee"]], ["Timing", ["attack", "release"]], ["Output", ["makeup", "mix"]], ["Character", ["char", "schp"]]],
  limiter: [["Limit", ["gain", "ceiling", "release"]], ["Stereo", ["link", "tp"]]],
  clipper: [["Clip", ["input", "ceiling", "knee"]], ["Quality", ["os", "delta"]], ["Output", ["mix", "output"]]],
  saturate: [["Color", ["mode", "drive", "bias", "tone"]], ["Output", ["mix", "output", "auto"]], ["Quality", ["os"]]],
  multi: [["Amount", ["depth", "time", "upward", "downward"]], ["Low", ["lowdown", "lowup", "lowgain"]], ["Mid", ["middown", "midup", "midgain"]],
    ["High", ["highdown", "highup", "highgain"]], ["Split", ["xlow", "xhigh"]], ["Gain", ["input", "output"]]],
  shaper: [["Shape", ["shape", "rate", "depth", "curve", "hold", "phase"]], ["Trigger", ["trig", "thresh"]], ["Band", ["band", "xover"]], ["Output", ["mix", "smooth"]]],
  // the selected band's knobs (the display picks the band)
  eqpro: (slot, st) => {
    const b = (st.band ?? 0) + 1;
    const type = pkReal(pkSpec("eqpro", `b${b}type`), slot[`b${b}type`] ?? PK_DEFAULTS.eqpro[`b${b}type`]);
    const cut = type === 3 || type === 4, flat = type >= 3;
    return [[`Band ${b}`, [`b${b}on`, `b${b}type`, `b${b}freq`, ...(flat ? [] : [`b${b}gain`]), `b${b}q`, ...(cut ? [`b${b}slope`] : []), `b${b}ch`]],
      ...(flat ? [] : [["Dynamic", [`b${b}dyn`, `b${b}thr`]]]), ["Output", ["output", "auto"]]];
  },
  delay: [["Echo", ["sync", "time", "feedback", "pingpong"]], ["Color", ["lowcut", "highcut", "drive", "wow"]], ["Mix", ["mix", "dry", "duck"]]],
  reverb: [["Room", ["type", "size", "damp", "width"]], ["Tone", ["predelay", "lowcut", "highcut", "mod"]], ["Mix", ["mix", "dry"]]],
  deess: [["Detect", ["freq", "thresh", "range"]], ["Mode", ["mode", "listen"]]],
  transient: [["Shape", ["attack", "sustain", "speed"]], ["Output", ["output", "mix"]]],
  width: [["Width", ["width", "lowwidth", "highwidth", "xover"]], ["Mono and balance", ["monobass", "balance", "output"]]],
  gate: [["Open", ["thresh", "range", "hyst"]], ["Timing", ["attack", "hold", "release"]], ["Listen to", ["schp", "sclp", "listen"]]],
  filter: [["Filter", ["type", "slope", "cutoff", "reso", "drive"]], ["Movement", ["rate", "shape", "lfo", "env"]], ["Mix", ["mix"]]],
  lofi: [["Digital", ["bits", "rate"]], ["Tape", ["wow", "flutter", "noise", "tone"]], ["Output", ["mix", "output"]]],
  utility: [["Level", ["gain", "pan", "width"]], ["Switches", ["phasel", "phaser", "mono", "swap", "dc"]]],
  doubler: [["Voices", ["voices", "detune", "delay", "width"]], ["Mix", ["mix", "lowcut"]]],
  autotune: [["Notes", ["key", "scale"]], ["Correction", ["speed", "humanize", "amount"]], ["Output", ["transpose", "mix"]]],
};

function pkSpec(kind, key) { return (PK_SPEC[kind] || []).find((s) => s[0] === key); }
function pkReal(spec, v) {
  const [, , k, a, b] = spec;
  if (k === "choice") return Math.round(v * (a.length - 1));
  if (k === "toggle") return v >= 0.5 ? 1 : 0;
  if (k === "sq") return a + (b - a) * v * v;
  return k === "log" ? a * (b / a) ** v : k === "lin" ? a + (b - a) * v : v * 100;
}
function pkNorm(spec, real) {
  const [, , k, a, b] = spec;
  if (k === "choice") return Math.max(0, Math.min(1, real / Math.max(1, a.length - 1)));
  if (k === "toggle") return real ? 1 : 0;
  if (k === "sq") return Math.max(0, Math.min(1, Math.sqrt(Math.max(0, (real - a) / (b - a)))));
  const v = k === "log" ? Math.log(Math.max(1e-9, real) / a) / Math.log(b / a) : k === "lin" ? (real - a) / (b - a) : real / 100;
  return Math.max(0, Math.min(1, v));
}
function pkFmt(spec, v) {
  const x = spec[6] || {};
  if ((x.offBelow != null && v < x.offBelow) || (x.offAbove != null && v >= x.offAbove)) return "off";
  if (spec[2] === "choice") return spec[3][pkReal(spec, v)];
  if (spec[2] === "toggle") return v >= 0.5 ? "On" : "Off";
  const r = pkReal(spec, v), u = spec[5] ?? "%";
  if (u === "x") return r.toFixed(r < 1 ? 2 : 1) + "x";
  if (u === "oct") return (r > 0.05 ? "+" : "") + r.toFixed(1) + " oct";
  if (u === "bit") return r.toFixed(1) + " bit";
  if (u === "ct") return r.toFixed(1) + " ct";
  if (u === "st") return (Math.round(r) > 0 ? "+" : "") + Math.round(r) + " st";
  if (u === "ms" && spec[2] === "sq" && r < 0.5) return "instant";
  if (u === "°") return Math.round(r) + "°";
  if (u === "Hz" && r < 10) return r.toFixed(2) + " Hz";
  if (u === "Hz") return r >= 1000 ? (r / 1000).toFixed(r >= 10000 ? 1 : 2) + " kHz" : Math.round(r) + " Hz";
  if (u === "ms") return r >= 1000 ? (r / 1000).toFixed(2) + " s" : r >= 10 ? Math.round(r) + " ms" : r.toFixed(1) + " ms";
  if (u === "dB") return (r > 0.05 ? "+" : "") + r.toFixed(1) + " dB";
  if (u === ":1") return r.toFixed(1) + ":1";
  if (u === "") return r.toFixed(2);
  return Math.round(r) + "%";
}

// ---------- factory presets ----------
const PK_PRESETS = {
  eq: { "Vocal presence": { lowcut: 0.3, low: 0.45, mid: 0.43, midfreq: 0.35, q: 0.4, high: 0.62, highfreq: 0.55 },
    "Kick punch": { lowcut: 0.05, low: 0.65, lowfreq: 0.3, mid: 0.35, midfreq: 0.3, q: 0.45, high: 0.58, highfreq: 0.4 },
    "Telephone": { lowcut: 0.6, highcut: 0.25, mid: 0.7, midfreq: 0.55, q: 0.3 },
    "Air": { high: 0.68, highfreq: 0.85 },
    "Clean up mud": { lowcut: 0.18, mid: 0.38, midfreq: 0.25, q: 0.35 } },
  comp: { "Gentle glue": { threshold: 0.6, ratio: 0.1, attack: 0.65, release: 0.55, makeup: 0.1 },
    "Vocal": { threshold: 0.45, ratio: 0.25, attack: 0.5, release: 0.45, makeup: 0.25 },
    "Drum smash": { threshold: 0.25, ratio: 0.8, attack: 0.45, release: 0.3, makeup: 0.45 },
    "Bass even": { threshold: 0.4, ratio: 0.35, attack: 0.55, release: 0.55, makeup: 0.25 },
    "Sidechain pump": { threshold: 0.3, ratio: 0.9, attack: 0.1, release: 0.5, makeup: 0 } },
  limiter: { "Transparent": { gain: 0.1, ceiling: 0.917, release: 0.5 }, "Loud master": { gain: 0.35, ceiling: 0.917, release: 0.35 },
    "Safety": { gain: 0, ceiling: 0.992, release: 0.6 } },
  delay: { "Slapback": { time: 0.05, feedback: 0.1, mix: 0.3, pingpong: 0 }, "Quarter echo": { time: 0.4, feedback: 0.35, mix: 0.3, pingpong: 0 },
    "Wide ping-pong": { time: 0.3, feedback: 0.45, mix: 0.35, pingpong: 1 }, "Dub": { time: 0.55, feedback: 0.75, mix: 0.4, pingpong: 0.5 } },
  reverb: { "Small room": { size: 0.25, damp: 0.5, mix: 0.2, width: 0.8 }, "Plate": { size: 0.55, damp: 0.25, mix: 0.3, width: 1 },
    "Big hall": { size: 0.9, damp: 0.35, mix: 0.35, width: 1 }, "Dark ambience": { size: 0.75, damp: 0.85, mix: 0.4, width: 1 } },
  drive: { "Warm": { drive: 0.25, tone: 0.4, level: 0.55 }, "Crunch": { drive: 0.55, tone: 0.6, level: 0.45 }, "Fuzz": { drive: 0.95, tone: 0.7, level: 0.35 } },
  chorus: { "Subtle": { rate: 0.15, depth: 0.3, mix: 0.3 }, "Lush": { rate: 0.25, depth: 0.7, mix: 0.5 }, "Vibrato": { rate: 0.5, depth: 0.6, mix: 1 } },
  tremolo: { "Slow": { rate: 0.15, depth: 0.5 }, "Choppy": { rate: 0.6, depth: 1 }, "Auto-pan": { rate: 0.2, depth: 0.7, stereo: 1 } },
};
const pkUserPresets = (kind) => store.get("presets." + kind, {});

// ---------- a knob ----------
// opts: label, value (0..1), def, spec, color, size, onChange(v), onGesture() (before a change starts: for undo)
function pkKnob(o) {
  const size = o.size || 54, el = document.createElement("div");
  el.className = "pkknob";
  el.tabIndex = 0;
  el.style.setProperty("--c", o.color);
  const r = size / 2 - 5, cx = size / 2, A0 = 135, SW = 270;
  const pt = (deg, rad) => [cx + rad * Math.cos(deg * Math.PI / 180), cx + rad * Math.sin(deg * Math.PI / 180)];
  const arc = (from, to, rad) => {
    const [x1, y1] = pt(from, rad), [x2, y2] = pt(to, rad);
    return `M${x1.toFixed(2)} ${y1.toFixed(2)}A${rad} ${rad} 0 ${to - from > 180 ? 1 : 0} 1 ${x2.toFixed(2)} ${y2.toFixed(2)}`;
  };
  // a bipolar knob (gain around 0) fills from the middle
  const bip = o.spec && o.spec[2] === "lin" && o.spec[3] < 0 && o.spec[4] > 0;
  const ticks = Array.from({ length: 11 }, (_, i) => { const [x1, y1] = pt(A0 + SW * i / 10, r + 3), [x2, y2] = pt(A0 + SW * i / 10, r + 5); return `M${x1} ${y1}L${x2} ${y2}`; }).join("");
  el.innerHTML = `<svg width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">
      <path d="${ticks}" stroke="rgba(255,255,255,0.14)" stroke-width="1"/>
      <path d="${arc(A0, A0 + SW, r)}" stroke="rgba(0,0,0,0.55)" stroke-width="5" fill="none" stroke-linecap="round"/>
      <path class="val" stroke="var(--c)" stroke-width="5" fill="none" stroke-linecap="round"/>
      <circle cx="${cx}" cy="${cx}" r="${r - 7}" class="cap"/>
      <line class="ptr" x1="${cx}" y1="${cx}" stroke="#fff" stroke-width="2.4" stroke-linecap="round"/>
    </svg><div class="pkv"></div><div class="pkl">${esc(o.label)}</div>`;
  const val = el.querySelector(".val"), ptr = el.querySelector(".ptr"), kv = el.querySelector(".pkv");
  let v = o.value;
  const render = () => {
    const deg = A0 + SW * v, mid = A0 + SW / 2;
    val.setAttribute("d", bip ? (Math.abs(v - 0.5) < 0.004 ? "" : v > 0.5 ? arc(mid, deg, r) : arc(deg, mid, r)) : v > 0.003 ? arc(A0, deg, r) : "");
    const [x, y] = pt(deg, r - 10);
    ptr.setAttribute("x2", x.toFixed(2)); ptr.setAttribute("y2", y.toFixed(2));
    kv.textContent = o.spec ? pkFmt(o.spec, v) : Math.round(v * 100) + "%";
  };
  const set = (nv) => {
    nv = Math.max(0, Math.min(1, nv));
    if (Math.abs(nv - v) < 1e-6) return;
    v = nv; render(); o.onChange(v);
  };
  el.setValue = (nv) => { v = Math.max(0, Math.min(1, nv)); render(); };
  el.midiSet = (nv) => { o.onGesture?.(); set(nv); };
  const svg = el.querySelector("svg");
  svg.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    if (e.altKey || e.ctrlKey || e.metaKey) { e.preventDefault(); typeIn(); return; }
    o.onGesture?.();
    svg.setPointerCapture(e.pointerId);
    el.classList.add("drag");
    let lastY = e.clientY;
    const move = (ev) => { set(v + (lastY - ev.clientY) * (ev.shiftKey ? 0.0012 : 0.006)); lastY = ev.clientY; };
    const up = () => { el.classList.remove("drag"); svg.removeEventListener("pointermove", move); svg.removeEventListener("pointerup", up); };
    svg.addEventListener("pointermove", move);
    svg.addEventListener("pointerup", up);
  });
  svg.addEventListener("dblclick", () => { o.onGesture?.(); set(o.def ?? 0.5); });
  el.addEventListener("wheel", (e) => { e.preventDefault(); o.onGesture?.(); set(v - Math.sign(e.deltaY) * (e.shiftKey ? 0.004 : 0.02)); }, { passive: false });
  el.addEventListener("keydown", (e) => {
    const d = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1 }[e.key];
    if (d) { e.preventDefault(); o.onGesture?.(); set(v + d * (e.shiftKey ? 0.004 : 0.02)); }
    if (e.key === "Enter") { e.preventDefault(); typeIn(); }
  });
  kv.addEventListener("click", typeIn);
  function typeIn() {
    if (!o.spec) return;
    const inp = document.createElement("input");
    inp.className = "pkin";
    inp.value = String(+pkReal(o.spec, v).toFixed(2));
    kv.replaceWith(inp);
    inp.focus(); inp.select();
    let over = false;
    const done = (ok) => {
      if (over) return;
      over = true;
      if (ok) {
        const n = parseFloat(inp.value.replace(",", ".").replace(/[^\d.\-+e]/g, ""));
        if (Number.isFinite(n)) {
          // "2k" means 2000
          const real = /k/i.test(inp.value) && o.spec[5] === "Hz" ? n * 1000 : n;
          o.onGesture?.(); set(pkNorm(o.spec, real));
        }
      }
      inp.replaceWith(kv); render();
    };
    inp.addEventListener("keydown", (e) => { if (e.key === "Enter") done(true); if (e.key === "Escape") done(false); e.stopPropagation(); });
    inp.addEventListener("blur", () => done(true));
  }
  render();
  return el;
}

// ---------- a choice and a switch (the same shape as a knob: setValue, onChange, onGesture) ----------
function pkChoice(o) {
  const opts = o.spec[3], el = document.createElement("div");
  el.className = "pkchoice";
  el.style.setProperty("--c", o.color);
  let v = o.value;
  const idx = () => Math.round(v * (opts.length - 1));
  // a few short options: side by side; more: a menu
  const seg = opts.length <= 4 && opts.join("").length <= 18;
  el.innerHTML = (seg ? `<div class="pkseg">${opts.map((x, i) => `<button data-i="${i}">${esc(x)}</button>`).join("")}</div>`
    : `<button class="pkdrop"></button>`) + `<div class="pkl">${esc(o.label)}</div>`;
  const pick = (i) => {
    const nv = opts.length > 1 ? i / (opts.length - 1) : 0;
    if (Math.abs(nv - v) < 1e-6) return;
    o.onGesture?.(); v = nv; render(); o.onChange(v);
  };
  const render = () => {
    if (seg) el.querySelectorAll("button").forEach((b) => b.classList.toggle("on", +b.dataset.i === idx()));
    else el.querySelector(".pkdrop").textContent = opts[idx()];
  };
  if (seg) el.querySelectorAll("button").forEach((b) => (b.onclick = () => pick(+b.dataset.i)));
  else el.querySelector(".pkdrop").onclick = (e) => {
    const r = e.currentTarget.getBoundingClientRect();
    showCtx(opts.map((x, i) => [(i === idx() ? "✓ " : "   ") + x, "", () => pick(i)]), r.left, r.bottom + 2);
  };
  el.addEventListener("wheel", (e) => { e.preventDefault(); pick(Math.max(0, Math.min(opts.length - 1, idx() + Math.sign(e.deltaY)))); }, { passive: false });
  el.setValue = (nv) => { v = nv; render(); };
  el.midiSet = (x) => pick(Math.round(Math.max(0, Math.min(1, x)) * (opts.length - 1)));
  render();
  return el;
}

function pkToggle(o) {
  const el = document.createElement("div");
  el.className = "pktoggle";
  el.style.setProperty("--c", o.color);
  el.innerHTML = `<button role="switch"><i></i></button><div class="pkl">${esc(o.label)}</div>`;
  const b = el.querySelector("button");
  let v = o.value;
  const render = () => { b.classList.toggle("on", v >= 0.5); b.setAttribute("aria-checked", v >= 0.5); };
  b.onclick = () => { o.onGesture?.(); v = v >= 0.5 ? 0 : 1; render(); o.onChange(v); };
  el.setValue = (nv) => { v = nv; render(); };
  el.midiSet = (x) => { const nv = x >= 0.5 ? 1 : 0; if (nv !== (v >= 0.5 ? 1 : 0)) { o.onGesture?.(); v = nv; render(); o.onChange(v); } };
  render();
  return el;
}

function pkControl(o) {
  return o.spec?.[2] === "choice" ? pkChoice(o) : o.spec?.[2] === "toggle" ? pkToggle(o) : pkKnob(o);
}

// ---------- a meter ----------
function pkMeter(label, kind = "level") {
  const el = document.createElement("div");
  el.className = "pkmeter " + kind;
  el.innerHTML = `<div class="bar"><i></i><b></b></div><span>${label}</span>`;
  const i = el.querySelector("i"), b = el.querySelector("b");
  let hold = 0, holdAt = 0;
  // level: 0..1 peak (shown -60..+6 dB); gr: dB turned down (0..24, from the top)
  el.set = (x) => {
    let frac;
    if (kind === "gr") frac = Math.min(1, x / 24);
    else { const db = x > 1e-6 ? 20 * Math.log10(x) : -90; frac = Math.max(0, Math.min(1, (db + 60) / 66)); }
    i.style.height = frac * 100 + "%";
    const now = performance.now();
    if (frac >= hold || now - holdAt > 1200) { hold = frac; holdAt = now; }
    b.style.bottom = kind === "gr" ? "" : hold * 100 + "%";
    b.style.top = kind === "gr" ? hold * 100 + "%" : "";
    el.classList.toggle("clip", kind !== "gr" && x >= 1);
  };
  return el;
}

// ---------- the window ----------
// w: the fx window record ({ el, t, pos }); slot: its rack slot (live object from the mixer)
function pkPanel(w, slot) {
  const kind = slot.kind, meta = FXMETA[kind], color = meta.color, t = w.t, pos = w.pos;
  const spec = PK_SPEC[kind] || [];
  const root = document.createElement("div");
  root.className = "pk";
  root.style.setProperty("--c", color);
  w.pk ||= { undo: [], redo: [], ab: null, which: "A", preset: null };
  const st = w.pk;
  const params = () => Object.fromEntries(spec.map(([k]) => [k, slot[k] ?? FX_DEFAULTS[kind][k]]));
  const knobs = {};
  // a change from a knob, a handle or a preset: into the engine and the mixer file
  const setParam = (k, v, from) => {
    slot[k] = v;
    api.rack_set(t, pos, k, v);
    if (from !== "knob") knobs[k]?.setValue(v);
    // a band that changes type gets the knobs that type has
    if (typeof PK_GROUPS[kind] === "function" && /type$/.test(k)) requestAnimationFrame(renderKnobs);
    display?.redraw?.();
    saveMixer();
  };
  const gesture = () => {
    const now = performance.now();
    if (st.lastGesture && now - st.lastGesture < 400) { st.lastGesture = now; return; } // one turn, one undo step
    st.lastGesture = now;
    st.undo.push(params()); st.redo = []; if (st.undo.length > 100) st.undo.shift();
    bar.refresh();
  };
  const applyAll = (vals, name) => {
    for (const [k, v] of Object.entries(vals)) if (pkSpec(kind, k)) setParam(k, v);
    if (name !== undefined) st.preset = name;
    bar.refresh();
  };

  // --- the bar: presets, A/B, undo, reset
  const bar = document.createElement("div");
  bar.className = "pkbar";
  const all = () => ({ ...(PK_PRESETS[kind] || {}), ...pkUserPresets(kind) });
  const names = () => Object.keys(all());
  bar.innerHTML = `<button class="pkb prev" title="Previous preset">◀</button><button class="pkpre" title="Presets"></button><button class="pkb next" title="Next preset">▶</button>
    <button class="pkb save" title="Save as a preset">Save</button><span class="sp"></span>
    <button class="pkb ab a" title="Setting A (click again to copy A to B)">A</button><button class="pkb ab b" title="Setting B">B</button>
    <button class="pkb undo" title="Undo (in this effect)">↶</button><button class="pkb redo" title="Redo">↷</button>
    <button class="pkb reset" title="Back to the defaults">Reset</button>`;
  const pre = bar.querySelector(".pkpre");
  const step = (d) => { const n = names(); if (!n.length) return; const i = (n.indexOf(st.preset) + d + n.length) % n.length; gesture(); applyAll(all()[n[i]], n[i]); };
  bar.querySelector(".prev").onclick = () => step(-1);
  bar.querySelector(".next").onclick = () => step(1);
  pre.onclick = (e) => {
    const r = pre.getBoundingClientRect(), user = pkUserPresets(kind);
    const items = Object.keys(PK_PRESETS[kind] || {}).map((n) => [n, "", () => { gesture(); applyAll(all()[n], n); }]);
    for (const n of Object.keys(user)) items.push(["★ " + n, "", () => { gesture(); applyAll(user[n], n); }]);
    if (Object.keys(user).length) items.push(["Delete a saved preset…", "", () => {
      const n = prompt("Which of your presets to delete?", st.preset && user[st.preset] ? st.preset : Object.keys(user)[0]);
      if (n && user[n]) { delete user[n]; store.set("presets." + kind, user); bar.refresh(); }
    }]);
    if (!items.length) items.push(["No presets yet", "", () => {}]);
    showCtx(items, r.left, r.bottom + 2);
  };
  bar.querySelector(".save").onclick = () => {
    const n = (prompt("A name for this setting:", st.preset || "My " + meta.name) || "").trim();
    if (!n) return;
    const user = pkUserPresets(kind);
    user[n] = params();
    store.set("presets." + kind, user);
    st.preset = n; bar.refresh();
    toast(`Saved "${n}" with your ${meta.name} presets.`);
  };
  const abBtn = (which) => bar.querySelector(".ab." + which.toLowerCase());
  for (const which of ["A", "B"]) abBtn(which).onclick = () => {
    st.ab ||= { A: params(), B: params() };
    if (st.which === which) { // again: copy this one to the other
      st.ab[which === "A" ? "B" : "A"] = params();
      toast(`${which} copied to ${which === "A" ? "B" : "A"}.`);
      return;
    }
    st.ab[st.which] = params();
    st.which = which;
    gesture(); applyAll(st.ab[which]);
  };
  bar.querySelector(".undo").onclick = () => { if (!st.undo.length) return; st.redo.push(params()); applyAll(st.undo.pop()); };
  bar.querySelector(".redo").onclick = () => { if (!st.redo.length) return; st.undo.push(params()); applyAll(st.redo.pop()); };
  bar.querySelector(".reset").onclick = () => { gesture(); applyAll(FX_DEFAULTS[kind], null); };
  bar.refresh = () => {
    pre.textContent = st.preset || "Presets";
    abBtn("A").classList.toggle("on", st.which === "A");
    abBtn("B").classList.toggle("on", st.which === "B");
    bar.querySelector(".undo").disabled = !st.undo.length;
    bar.querySelector(".redo").disabled = !st.redo.length;
  };
  bar.refresh();
  root.append(bar);

  // --- the display and the meters
  const mid = document.createElement("div");
  mid.className = "pkmid";
  st.band ??= 0;
  const display = PK_DISPLAY[kind]?.(slot, color, setParam, gesture, t, pos, st);
  if (display) mid.append(display);
  const meters = document.createElement("div");
  meters.className = "pkmeters";
  const mIn = pkMeter("In"), mOut = pkMeter("Out"), mGr = PK_GR.has(kind) ? pkMeter("GR", "gr") : null;
  meters.append(mIn, ...(mGr ? [mGr] : []), mOut);
  mid.append(meters);
  root.append(mid);
  w.meter = (m) => { mIn.set(m[0]); mOut.set(m[1]); mGr?.set(m[2]); display?.live?.(m); };
  w.scope = display?.scope ? (sc) => display.scope(sc) : null;
  if (display) display.select = (b) => { st.band = b; renderKnobs(); display.redraw?.(); };
  root.classList.add(kind);

  // --- the knobs, grouped
  const row = document.createElement("div");
  row.className = "pkknobs";
  root.append(row);
  function renderKnobs() {
  row.replaceChildren();
  for (const k of Object.keys(knobs)) delete knobs[k];
  const g0 = PK_GROUPS[kind];
  const groups = typeof g0 === "function" ? g0(slot, st) : g0 || [["", spec.map((s) => s[0])]];
  for (const [title, keys] of groups) {
    const g = document.createElement("div");
    g.className = "pkgroup";
    if (title) g.innerHTML = `<div class="pkgt">${esc(title)}</div>`;
    const ks = document.createElement("div");
    ks.className = "pkgk";
    for (const k of keys) {
      const s = pkSpec(kind, k);
      if (!s) continue;
      const kn = pkControl({ label: s[1], value: slot[k] ?? FX_DEFAULTS[kind][k], def: FX_DEFAULTS[kind][k], spec: s, color,
        onGesture: gesture, onChange: (v) => setParam(k, v, "knob") });
      knobs[k] = kn;
      ks.append(autoOnControl(kn, t, `fx:${pos}:${kind}:${k}`));
    }
    g.append(ks);
    row.append(g);
  }
  }
  renderKnobs();
  // what only some effects have: the compressor's sidechain, errors
  if (PK_KEYED.has(kind) && (t[0] === "T" || t === "M")) root.append(keySelect(t, pos, slot));
  if (slot.error) root.insertAdjacentHTML("afterbegin", `<div class="mnote bad">${esc(slot.error)}</div>`);
  root.classList.toggle("off", !slot.on);
  return root;
}

// effects that show how much they turn down, and that can listen to another track
const PK_GR = new Set(["comp", "limiter", "clipper", "multi", "shaper", "deess", "gate"]);
const PK_KEYED = new Set(["comp", "shaper", "gate"]);

// what the displays draw from the engine (spectra, band levels), 25 times a second
setInterval(async () => {
  if (typeof fxw === "undefined" || !fxw.size || !api.rack_scope) return;
  for (const w of fxw.values()) {
    if (!w.scope) continue;
    try { w.scope(await api.rack_scope(w.t, w.pos)); } catch (e) {}
  }
}, 40);

// the meters of every open window, a dozen times a second
setInterval(async () => {
  if (typeof fxw === "undefined" || !fxw.size || !api.rack_meters) return;
  const strips = new Set([...fxw.values()].filter((w) => w.meter).map((w) => w.t));
  for (const t of strips) {
    let ms;
    try { ms = await api.rack_meters(t); } catch (e) { continue; }
    for (const w of fxw.values()) if (w.t === t && w.meter && ms[w.pos]) w.meter(ms[w.pos]);
  }
}, 80);

// ---------- the displays ----------
function pkCanvas(cls, draw, h = 150) {
  const cv = document.createElement("canvas");
  cv.className = "pkdisp " + cls;
  cv.style.height = h + "px";
  cv.redraw = () => {
    const w = cv.clientWidth || 360, hh = cv.clientHeight || h;
    const ctx = sizeCanvas(cv, w, hh, false);
    const g = ctx.createLinearGradient(0, 0, 0, hh);
    g.addColorStop(0, "#15131c"); g.addColorStop(1, "#0b0a0f");
    ctx.fillStyle = g; ctx.fillRect(0, 0, w, hh);
    draw(ctx, w, hh);
  };
  requestAnimationFrame(() => cv.redraw());
  new ResizeObserver(() => cv.redraw()).observe(cv);
  return cv;
}
const glow = (ctx, color, blur = 10) => { ctx.shadowColor = color; ctx.shadowBlur = blur; };
const noGlow = (ctx) => { ctx.shadowBlur = 0; };

const PK_DISPLAY = {
  // the EQ: the curve, with a handle per band to drag (frequency and gain; scroll for Q)
  eq(slot, color, setParam, gesture) {
    const sp = (k) => pkSpec("eq", k);
    const RANGE = 18;
    const bands = [
      { f: "lowcut", g: null, label: "LC" }, { f: "lowfreq", g: "low", label: "L" },
      { f: "midfreq", g: "mid", label: "M", q: "q" }, { f: "highfreq", g: "high", label: "H" }, { f: "highcut", g: null, label: "HC" },
    ];
    let drag = null, hover = null;
    const geo = (w, h) => ({ x: (f) => Math.log(f / 20) / Math.log(1000) * w, fx: (px) => 20 * 1000 ** (px / w), y: (db) => h / 2 - db / RANGE * (h / 2 - 10), dby: (py) => (h / 2 - py) / (h / 2 - 10) * RANGE });
    const pos = (b, w, h) => {
      const G = geo(w, h), v = slot[b.f] ?? FX_DEFAULTS.eq[b.f];
      let f = pkReal(sp(b.f), v);
      if (b.f === "lowcut" && v < 0.005) f = 20; if (b.f === "highcut" && v >= 0.995) f = 20000;
      const db = b.g ? pkReal(sp(b.g), slot[b.g] ?? 0.5) : 0;
      return [Math.max(9, Math.min(w - 9, G.x(f))), G.y(db)];
    };
    const cv = pkCanvas("eqd", (ctx, w, h) => {
      const G = geo(w, h);
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (const f of [50, 100, 200, 500, 1000, 2000, 5000, 10000]) ctx.fillRect(Math.round(G.x(f)), 0, 1, h);
      for (const db of [-12, -6, 0, 6, 12]) ctx.fillRect(0, Math.round(G.y(db)), w, 1);
      ctx.fillStyle = "rgba(255,255,255,0.35)"; ctx.font = "600 9px system-ui, sans-serif";
      for (const [f, l] of [[100, "100"], [1000, "1k"], [10000, "10k"]]) ctx.fillText(l, G.x(f) + 3, h - 4);
      for (const db of [-12, 12]) ctx.fillText((db > 0 ? "+" : "") + db, 3, G.y(db) - 2);
      const coefs = eqCoefs(Object.fromEntries(PK_SPEC.eq.map(([k]) => [k, slot[k] ?? FX_DEFAULTS.eq[k]])), ui.rate || 48000);
      ctx.beginPath();
      for (let px = 0; px <= w; px++) { const db = Math.max(-RANGE - 4, Math.min(RANGE + 4, eqDb(coefs, G.fx(px), ui.rate || 48000))); px ? ctx.lineTo(px, G.y(db)) : ctx.moveTo(px, G.y(db)); }
      const fill = ctx.createLinearGradient(0, 0, 0, h);
      fill.addColorStop(0, color + "55"); fill.addColorStop(1, color + "08");
      ctx.save(); ctx.lineTo(w, G.y(0)); ctx.lineTo(0, G.y(0)); ctx.closePath(); ctx.fillStyle = fill; ctx.fill(); ctx.restore();
      ctx.beginPath();
      for (let px = 0; px <= w; px++) { const db = Math.max(-RANGE - 4, Math.min(RANGE + 4, eqDb(coefs, G.fx(px), ui.rate || 48000))); px ? ctx.lineTo(px, G.y(db)) : ctx.moveTo(px, G.y(db)); }
      glow(ctx, color, 12); ctx.strokeStyle = color; ctx.lineWidth = 2.2; ctx.stroke(); noGlow(ctx);
      for (const b of bands) {
        const [x, y] = pos(b, w, h), on = hover === b || drag?.b === b;
        ctx.beginPath(); ctx.arc(x, y, on ? 8 : 6.5, 0, 2 * Math.PI);
        ctx.fillStyle = on ? "#fff" : color; glow(ctx, color, on ? 14 : 6); ctx.fill(); noGlow(ctx);
        ctx.fillStyle = on ? "#16141c" : "#0b0a0f"; ctx.font = "800 8.5px system-ui, sans-serif"; ctx.textAlign = "center";
        ctx.fillText(b.label, x, y + 3); ctx.textAlign = "left";
      }
      if (hover || drag) {
        const b = drag?.b || hover, [x, y] = pos(b, w, h);
        const txt = pkFmt(sp(b.f), slot[b.f] ?? FX_DEFAULTS.eq[b.f]) + (b.g ? "  " + pkFmt(sp(b.g), slot[b.g] ?? 0.5) : "") + (b.q ? "  Q " + pkFmt(sp("q"), slot.q ?? FX_DEFAULTS.eq.q) : "");
        ctx.font = "700 10.5px system-ui, sans-serif";
        const tw = ctx.measureText(txt).width + 12, tx = Math.min(w - tw - 2, Math.max(2, x - tw / 2)), ty = y < 30 ? y + 14 : y - 26;
        ctx.fillStyle = "rgba(10,9,14,0.88)"; ctx.fillRect(tx, ty, tw, 18);
        ctx.fillStyle = "#fff"; ctx.fillText(txt, tx + 6, ty + 13);
      }
    }, 170);
    const hit = (e) => {
      const r = cv.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
      let best = null, bd = 14;
      for (const b of bands) { const [x, y] = pos(b, r.width, r.height), d = Math.hypot(x - px, y - py); if (d < bd) { bd = d; best = b; } }
      return { b: best, px, py, w: r.width, h: r.height };
    };
    cv.addEventListener("pointermove", (e) => {
      if (drag) return;
      const hb = hit(e).b;
      if (hb !== hover) { hover = hb; cv.style.cursor = hb ? "grab" : "default"; cv.redraw(); }
    });
    cv.addEventListener("pointerleave", () => { hover = null; cv.redraw(); });
    cv.addEventListener("pointerdown", (e) => {
      const hh = hit(e);
      if (!hh.b) return;
      cv.setPointerCapture(e.pointerId);
      gesture();
      drag = { b: hh.b };
      const move = (ev) => {
        const q = hit(ev), G = geo(q.w, q.h), b = drag.b;
        const fq = G.fx(Math.max(0, Math.min(q.w, q.px)));
        setParam(b.f, pkNorm(sp(b.f), fq));
        if (b.g && !ev.shiftKey) setParam(b.g, pkNorm(sp(b.g), Math.max(-15, Math.min(15, G.dby(q.py)))));
        cv.redraw();
      };
      const up = () => { drag = null; cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up); cv.redraw(); };
      cv.addEventListener("pointermove", move); cv.addEventListener("pointerup", up);
    });
    cv.addEventListener("dblclick", (e) => { const hh = hit(e); if (!hh.b) return; gesture(); setParam(hh.b.f, FX_DEFAULTS.eq[hh.b.f]); if (hh.b.g) setParam(hh.b.g, 0.5); });
    cv.addEventListener("wheel", (e) => {
      const hh = hit(e);
      if (!hh.b || !hh.b.q) return;
      e.preventDefault(); gesture();
      setParam("q", Math.max(0, Math.min(1, (slot.q ?? FX_DEFAULTS.eq.q) - Math.sign(e.deltaY) * 0.03)));
    }, { passive: false });
    cv.title = "Drag a handle: frequency and gain (Shift: frequency only). Scroll on M: its width (Q). Double-click: back to flat.";
    return cv;
  },

  // the compressor: input level against output level, the signal as a dot on it, and the
  // gain reduction over the last seconds
  comp(slot, color) {
    const hist = [];
    let lastIn = 0;
    const cv = pkCanvas("compd", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("comp", k), slot[k] ?? FX_DEFAULTS.comp[k]);
      const thr = g("threshold"), ratio = g("ratio"), mk = g("makeup");
      const sz = h - 16, ox = 8, oy = 8, lo = -60, hi = 0;
      const X = (db) => ox + (db - lo) / (hi - lo) * sz, Y = (db) => oy + sz - (Math.max(lo, Math.min(hi + 18, db)) - lo) / (hi - lo) * sz;
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (let d = -60; d <= 0; d += 12) { ctx.fillRect(Math.round(X(d)), oy, 1, sz); ctx.fillRect(ox, Math.round(Y(d)), sz, 1); }
      ctx.strokeStyle = "rgba(255,255,255,0.12)"; ctx.beginPath(); ctx.moveTo(X(lo), Y(lo)); ctx.lineTo(X(hi), Y(hi)); ctx.stroke();
      const out = (i) => (i <= thr ? i : thr + (i - thr) / ratio) + mk;
      ctx.beginPath();
      for (let i = lo; i <= hi; i += 0.5) i === lo ? ctx.moveTo(X(i), Y(out(i))) : ctx.lineTo(X(i), Y(out(i)));
      glow(ctx, color, 10); ctx.strokeStyle = color; ctx.lineWidth = 2.2; ctx.stroke(); noGlow(ctx);
      ctx.fillStyle = color + "aa"; ctx.fillRect(X(thr) - 0.5, oy, 1, sz);
      // the signal now
      const inDb = lastIn > 1e-6 ? 20 * Math.log10(lastIn) : lo;
      if (inDb > lo) { ctx.beginPath(); ctx.arc(X(Math.min(hi, inDb)), Y(out(Math.min(hi, inDb))), 5, 0, 2 * Math.PI); ctx.fillStyle = "#fff"; glow(ctx, "#fff", 12); ctx.fill(); noGlow(ctx); }
      // gain reduction history, to the right
      const hx = ox + sz + 14, hw = w - hx - 6;
      if (hw > 30) {
        ctx.fillStyle = "rgba(255,255,255,0.04)"; ctx.fillRect(hx, oy, hw, sz);
        ctx.beginPath(); ctx.moveTo(hx, oy);
        const HX = (k) => hx + hw - (hist.length - 1 - k) / 199 * hw;
        ctx.moveTo(HX(0), oy);
        hist.forEach((gr, k) => ctx.lineTo(HX(k), oy + Math.min(sz, gr / 24 * sz)));
        ctx.lineTo(hx + hw, oy); ctx.closePath();
        ctx.fillStyle = "#ff7a6b66"; ctx.fill();
        ctx.fillStyle = "rgba(255,255,255,0.45)"; ctx.font = "600 9px system-ui, sans-serif";
        ctx.fillText("Gain reduction", hx + 4, oy + sz - 4);
        ctx.fillText(`−${(hist[hist.length - 1] || 0).toFixed(1)} dB`, hx + hw - 44, oy + 11);
      }
      ctx.fillStyle = "rgba(255,255,255,0.45)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText(`${thr.toFixed(0)} dB · ${ratio.toFixed(1)}:1`, ox + 4, oy + 11);
    }, 170);
    cv.live = (m) => { lastIn = m[0]; hist.push(m[2]); if (hist.length > 200) hist.shift(); cv.redraw(); };
    return cv;
  },

  // the limiter: gain reduction over time under the ceiling
  limiter(slot, color) {
    const hist = [], outs = [];
    const cv = pkCanvas("limd", (ctx, w, h) => {
      const ceil = pkReal(pkSpec("limiter", "ceiling"), slot.ceiling ?? FX_DEFAULTS.limiter.ceiling);
      const Y = (db) => 8 + (-db) / 24 * (h - 16);
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (let d = 0; d >= -24; d -= 6) ctx.fillRect(0, Math.round(Y(d)), w, 1);
      // the output level as it went
      const HX = (k) => w - (outs.length - 1 - k) / 199 * w;
      ctx.beginPath(); ctx.moveTo(HX(0), h);
      outs.forEach((o, k) => ctx.lineTo(HX(k), Y(Math.max(-24, o > 1e-6 ? 20 * Math.log10(o) : -24))));
      ctx.lineTo(w, h); ctx.closePath();
      ctx.fillStyle = color + "33"; ctx.fill();
      // and how much was taken off, from the top
      ctx.beginPath(); ctx.moveTo(HX(0), Y(0));
      hist.forEach((gr, k) => ctx.lineTo(HX(k), Y(-Math.min(24, gr))));
      glow(ctx, "#ff7a6b", 8); ctx.strokeStyle = "#ff7a6b"; ctx.lineWidth = 2; ctx.stroke(); noGlow(ctx);
      ctx.fillStyle = color; ctx.fillRect(0, Math.round(Y(ceil)), w, 1.5);
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText(`Ceiling ${ceil.toFixed(1)} dB`, 6, Y(ceil) - 3);
      ctx.fillText(`−${(hist[hist.length - 1] || 0).toFixed(1)} dB`, w - 44, 14);
    }, 150);
    cv.live = (m) => { hist.push(m[2]); outs.push(m[1]); if (hist.length > 200) { hist.shift(); outs.shift(); } cv.redraw(); };
    return cv;
  },

  // the delay: the echoes as they come, left and right
  delay(slot, color) {
    return pkCanvas("dlyd", (ctx, w, h) => {
      const g = (k) => slot[k] ?? FX_DEFAULTS.delay[k];
      const sync = pkReal(pkSpec("delay", "sync"), g("sync") ?? 0), beats = [0.125, 1 / 6, 0.25, 0.375, 1 / 3, 0.5, 0.75, 2 / 3, 1, 1.5, 2, 4][sync - 1];
      const bpm = (typeof song !== "undefined" && song.view?.meta?.bpm) || 120;
      const ms = sync ? beats * 60000 / bpm : pkReal(pkSpec("delay", "time"), g("time")), fb = g("feedback"), pp = g("pingpong"), mix = g("mix");
      const span = Math.max(2000, ms * 6), X = (t) => 10 + t / span * (w - 20), mid = h / 2;
      ctx.fillStyle = "rgba(255,255,255,0.06)"; ctx.fillRect(0, mid, w, 1);
      ctx.fillStyle = "rgba(255,255,255,0.4)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText("L", 4, 14); ctx.fillText("R", 4, h - 6);
      const full = h / 2 - 12, dry = g("dry");
      ctx.fillStyle = "#fff"; ctx.fillRect(X(0) - 1.5, mid - full * dry, 3, 2 * full * dry);
      // each echo: the left and right halves, ping-pong moves them from side to side
      let a = 0.3 + 0.7 * mix;
      const env = [];
      for (let k = 1; ms * k < span && a > 0.01; k++, a *= fb) {
        const x = X(ms * k), toL = k % 2 ? 1 : 1 - pp, toR = k % 2 ? 1 - pp : 1;
        env.push([x, a]);
        glow(ctx, color, 10); ctx.fillStyle = color;
        ctx.fillRect(x - 2.5, mid - full * a * toL, 5, full * a * toL);
        ctx.fillStyle = color + "aa";
        ctx.fillRect(x - 2.5, mid, 5, full * a * toR);
        noGlow(ctx);
      }
      if (env.length > 1) {
        ctx.beginPath(); env.forEach(([x, a], k) => (k ? ctx.lineTo(x, mid - full * a) : ctx.moveTo(x, mid - full * a)));
        ctx.setLineDash([3, 4]); ctx.strokeStyle = color + "88"; ctx.lineWidth = 1; ctx.stroke(); ctx.setLineDash([]);
      }
      ctx.fillStyle = "rgba(255,255,255,0.5)";
      ctx.textAlign = "right";
      ctx.fillText(`${sync ? pkFmt(pkSpec("delay", "sync"), g("sync")) + " · " : ""}${Math.round(ms)} ms · ${Math.round(fb * 100)}% feedback`, w - 8, 14);
      ctx.textAlign = "left";
    }, 130);
  },

  // the reverb: early reflections, then the tail as it fades (darker with damping)
  reverb(slot, color) {
    return pkCanvas("revd", (ctx, w, h) => {
      const g = (k) => slot[k] ?? FX_DEFAULTS.reverb[k];
      const size = g("size"), damp = g("damp"), width = g("width"), mix = g("mix");
      const type = pkReal(pkSpec("reverb", "type"), g("type") ?? 0), pre = 0.2 * (g("predelay") ?? 0);
      const rt = [0.3 + size * 4.7, 0.25 + 1.6 * size * size, 0.8 + 8 * size * size, 0.5 + 4.5 * size * size][type];
      const len = Math.min(1, 0.15 + rt / 8), base = h - 12, x0 = 12 + pre / 2 * (w - 20);
      const grad = ctx.createLinearGradient(0, 0, w, 0);
      grad.addColorStop(0, color + "cc"); grad.addColorStop(1, damp > 0.5 ? "#30264066" : color + "22");
      ctx.beginPath(); ctx.moveTo(x0, base);
      for (let x = x0; x <= w - 8; x++) {
        const t = (x - x0) / (w - 20), env = Math.exp(-t / len * 3.2) * mix * 1.6;
        const jitter = 0.75 + 0.25 * Math.sin(x * 1.7) * Math.sin(x * 0.31);
        ctx.lineTo(x, base - Math.min(1, env * jitter) * (h - 24));
      }
      ctx.lineTo(w - 8, base); ctx.closePath(); ctx.fillStyle = grad; ctx.fill();
      for (let k = 0; k < 9; k++) { // early reflections
        if (type === 3) break; // a plate has no early reflections
        const t = 0.01 + k * 0.012 * (0.5 + size), x = x0 + t * (w - 20), a = (1 - k / 10) * mix * 1.4;
        ctx.fillStyle = "#fff"; ctx.fillRect(x, base - Math.min(1, a) * (h - 24), 2, Math.min(1, a) * (h - 24));
      }
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.textAlign = "right";
      ctx.fillText(`${["Classic", "Room", "Hall", "Plate"][type]} · ${rt.toFixed(1)} s tail${pre > 0.001 ? ` · ${Math.round(pre * 1000)} ms pre-delay` : ""} · width ${Math.round(width * 100)}%`, w - 8, 14);
      ctx.textAlign = "left";
    }, 130);
  },

  // drive: the shape the sound is bent into
  drive(slot, color) {
    return pkCanvas("drvd", (ctx, w, h) => {
      const d = slot.drive ?? 0.4, k = 1 + 40 * d * d, sz = h - 16, ox = (w - sz) / 2, oy = 8, nrm = 1 / Math.sqrt(Math.tanh(k));
      ctx.fillStyle = "rgba(255,255,255,0.05)"; ctx.fillRect(ox + sz / 2, oy, 1, sz); ctx.fillRect(ox, oy + sz / 2, sz, 1);
      ctx.strokeStyle = "rgba(255,255,255,0.12)"; ctx.beginPath(); ctx.moveTo(ox, oy + sz); ctx.lineTo(ox + sz, oy); ctx.stroke();
      ctx.beginPath();
      for (let i = 0; i <= 100; i++) { const x = i / 50 - 1, y = Math.max(-1.15, Math.min(1.15, Math.tanh(k * x) * nrm / 1.15)); i ? ctx.lineTo(ox + (x + 1) / 2 * sz, oy + (1 - y) / 2 * sz) : ctx.moveTo(ox, oy + (1 - y) / 2 * sz); }
      glow(ctx, color, 10); ctx.strokeStyle = color; ctx.lineWidth = 2.4; ctx.stroke(); noGlow(ctx);
    }, 130);
  },

  // the amp: its model (load, swap, what it is)
  nam(slot, color, setParam, gesture, t, pos) { const d = namSection(t, pos, slot); d.classList.add("pkdisp", "pknam"); return d; },

  // chorus and tremolo: their LFO, moving
  chorus(slot, color) { return pkLfo(slot, color, "chorus"); },
  tremolo(slot, color) { return pkLfo(slot, color, "tremolo"); },
};

function pkLfo(slot, color, kind) {
  const t0 = performance.now();
  const cv = pkCanvas("lfod", (ctx, w, h) => {
    const rate = pkReal(pkSpec(kind, "rate"), slot.rate ?? FX_DEFAULTS[kind].rate), depth = slot.depth ?? 0.5;
    const ph = (performance.now() - t0) / 1000 * rate * 2 * Math.PI, mid = h / 2;
    ctx.fillStyle = "rgba(255,255,255,0.06)"; ctx.fillRect(0, mid, w, 1);
    for (const [side, c] of [[0, color], [kind === "tremolo" ? Math.PI * (slot.stereo ?? 0) : Math.PI / 2, "#ffffff70"]]) {
      ctx.beginPath();
      for (let x = 0; x <= w; x++) { const y = mid - Math.sin(x / w * 4 * Math.PI - ph + side) * depth * (h / 2 - 10); x ? ctx.lineTo(x, y) : ctx.moveTo(x, y); }
      glow(ctx, c, 8); ctx.strokeStyle = c; ctx.lineWidth = 2; ctx.stroke(); noGlow(ctx);
    }
    ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
    ctx.fillText(`${rate.toFixed(2)} Hz`, 6, 14);
  }, 110);
  cv.live = () => cv.redraw();
  return cv;
}

// ======================================================================================
// Duet's own plugins (engine: plugins.rs): their entries in the effect list, defaults,
// presets and displays.
// ======================================================================================

// the defaults, the same as fx.rs
const EQP_FREQS = [50, 120, 300, 800, 2000, 5000, 10000, 16000];
const PK_DEFAULTS = {
  eqpro: Object.assign({ output: 0.5, auto: 0 }, ...EQP_FREQS.map((f, i) => {
    const b = i + 1;
    return { [`b${b}on`]: 0, [`b${b}type`]: (b === 1 ? 3 : b === 8 ? 4 : 0) / 6, [`b${b}freq`]: Math.log(f / 20) / Math.log(1000), [`b${b}gain`]: 0.5,
      [`b${b}q`]: Math.log(0.7071 / 0.1) / Math.log(180), [`b${b}slope`]: 0.5, [`b${b}ch`]: 0, [`b${b}dyn`]: 0.5, [`b${b}thr`]: 0.6 };
  })),
  clipper: { input: 0, ceiling: 1, knee: 0.3, os: 0.667, mix: 1, output: 0.5, delta: 0 },
  multi: { depth: 0.5, time: 0.5, upward: 1, downward: 1, input: 0.3333, output: 0.5, xlow: 0.2493, xhigh: 0.3697,
    lowdown: 0.5, lowup: 0.3333, lowgain: 0.5, middown: 0.5, midup: 0.3333, midgain: 0.5, highdown: 0.5, highup: 0.3333, highgain: 0.5 },
  saturate: { drive: 0.25, mode: 0, bias: 0, tone: 0.5, mix: 1, output: 0.5, os: 0.333, auto: 1 },
  shaper: { depth: 1, rate: 0.5, shape: 0, curve: 0.5, hold: 0.1, phase: 0, trig: 0, thresh: 0.5, band: 0, xover: 0.3486, mix: 1, smooth: 0 },
};
Object.assign(PK_DEFAULTS, {
  deess: { freq: 0.6578, thresh: 0.5, range: 0.4167, mode: 0, listen: 0 },
  transient: { attack: 0.5, sustain: 0.5, speed: 0.5, output: 0.5, mix: 1 },
  width: { width: 0.5, lowwidth: 0.5, highwidth: 0.5, xover: 0.4307, monobass: 0, balance: 0.5, output: 0.5 },
  gate: { thresh: 0.5, range: 1, attack: 0.3333, hold: 0.04, release: 0.5304, hyst: 0.3333, schp: 0, sclp: 1, listen: 0 },
  filter: { type: 0, slope: 0, cutoff: 0.6, reso: 0.3, drive: 0, rate: 0.5, shape: 0, lfo: 0.5, env: 0.5, mix: 1 },
  lofi: { bits: 0.5, rate: 0.6, wow: 0.2, flutter: 0.1, noise: 0.15, tone: 0.7, mix: 1, output: 0.5 },
  utility: { gain: 0.5, pan: 0.5, width: 0.5, phasel: 0, phaser: 0, mono: 0, swap: 0, dc: 0 },
  doubler: { detune: 0.24, delay: 0.4, width: 0.8, voices: 0, mix: 0.5, lowcut: 0 },
  autotune: { key: 0, scale: 1 / 6, speed: 0.25, humanize: 0.3, amount: 1, transpose: 0.5, mix: 1 },
});
const PK_NEW_DEFAULTS = { comp: { knee: 0, mix: 1, schp: 0, char: 0 }, limiter: { link: 1, tp: 1 },
  delay: { sync: 0, lowcut: 0, highcut: 1, drive: 0, wow: 0, duck: 0 }, reverb: { type: 0, predelay: 0, lowcut: 0, highcut: 1, mod: 0.3 } };
const pkKnobs = (kind) => PK_SPEC[kind].map((s) => [s[0], s[1], (v) => pkFmt(s, v)]);
const PK_FX = [
  { id: "eqpro", name: "EQ Pro", sub: "8-band, dynamic, M/S", color: "#4fd1ff", knobs: pkKnobs("eqpro"), group: "EQ" },
  { id: "multi", name: "Multiband", sub: "up and down, 3 bands", color: "#ff8a3d", knobs: pkKnobs("multi"), group: "Dynamics" },
  { id: "clipper", name: "Clipper", sub: "loud without overs", color: "#ff4d6d", knobs: pkKnobs("clipper"), group: "Dynamics" },
  { id: "saturate", name: "Saturator", sub: "tape, tube, fuzz", color: "#ffb347", knobs: pkKnobs("saturate"), group: "Color" },
  { id: "shaper", name: "Shaper", sub: "pump in time", color: "#7cf29a", knobs: pkKnobs("shaper"), group: "Movement" },
  { id: "deess", name: "De-esser", sub: "tames the s's", color: "#c3a6ff", knobs: pkKnobs("deess") },
  { id: "transient", name: "Transient", sub: "more or less punch", color: "#ff9f6b", knobs: pkKnobs("transient") },
  { id: "width", name: "Imager", sub: "stereo width, mono bass", color: "#5ee0d0", knobs: pkKnobs("width") },
  { id: "gate", name: "Gate", sub: "silence between hits", color: "#9ad04f", knobs: pkKnobs("gate") },
  { id: "filter", name: "Filter", sub: "sweeps and wobbles", color: "#ff6bd6", knobs: pkKnobs("filter") },
  { id: "lofi", name: "Lo-fi", sub: "bits, tape, vinyl", color: "#d6b07a", knobs: pkKnobs("lofi") },
  { id: "doubler", name: "Doubler", sub: "wide copies", color: "#7fb2ff", knobs: pkKnobs("doubler") },
  { id: "autotune", name: "Tune live", sub: "pitch correction", color: "#ff5fa2", knobs: pkKnobs("autotune") },
  { id: "utility", name: "Utility", sub: "gain, pan, phase, mono", color: "#b8b8c8", knobs: pkKnobs("utility") },
];

// presets in real units, made into knob values
function pkPre(kind, real) {
  const out = {};
  for (const [k, v] of Object.entries(real)) {
    const s = pkSpec(kind, k);
    out[k] = typeof v === "string" ? pkNorm(s, s[3].indexOf(v)) : s[2] === "toggle" || s[2] === "pct" ? (s[2] === "pct" ? v / 100 : v) : pkNorm(s, v);
  }
  return out;
}
// an EQ Pro setting: [type, Hz, dB, Q, extra] per band, the rest off
function eqpPre(bands, extra = {}) {
  const out = {};
  for (let b = 1; b <= 8; b++) out[`b${b}on`] = 0;
  bands.forEach(([type, f, g, q, x = {}], i) => {
    const b = i + 1;
    Object.assign(out, pkPre("eqpro", { [`b${b}on`]: 1, [`b${b}type`]: type, [`b${b}freq`]: f, [`b${b}gain`]: g, [`b${b}q`]: q,
      ...Object.fromEntries(Object.entries(x).map(([k, v]) => [`b${b}${k}`, v])) }));
  });
  return Object.assign(out, pkPre("eqpro", extra));
}
Object.assign(PK_PRESETS, {
  eqpro: {
    "Vocal clarity": eqpPre([["Low cut", 90, 0, 0.71], ["Bell", 280, -3, 1.4], ["Bell", 3200, 2.5, 0.9], ["High shelf", 11000, 3, 0.7]]),
    "De-mud (dynamic)": eqpPre([["Bell", 320, 0, 1.2, { dyn: -6, thr: -30 }]]),
    "Kick + bass room": eqpPre([["Low cut", 28, 0, 0.71, { slope: "48 dB" }], ["Bell", 60, 2, 1], ["Bell", 400, -4, 1.6], ["Bell", 3500, 2, 1.5]]),
    "Wide top (side)": eqpPre([["Low cut", 150, 0, 0.71, { ch: "Side" }], ["High shelf", 9000, 3, 0.7, { ch: "Side" }]]),
    "Master polish": eqpPre([["Low cut", 25, 0, 0.71], ["Low shelf", 90, 1, 0.7], ["Bell", 2500, -1, 0.8], ["High shelf", 14000, 1.5, 0.7]]),
    "Telephone": eqpPre([["Low cut", 500, 0, 0.71, { slope: "48 dB" }], ["Bell", 1500, 6, 1.5], ["High cut", 3000, 0, 0.71, { slope: "48 dB" }]]),
    "De-ess (dynamic)": eqpPre([["Bell", 6500, 0, 2.5, { dyn: -9, thr: -32 }]]),
  },
  clipper: {
    "Drum bus smash": pkPre("clipper", { input: 6, ceiling: -1, knee: 20, os: "4x" }),
    "808 grit": pkPre("clipper", { input: 9, ceiling: -3, knee: 0, os: "2x" }),
    "Master transparent": pkPre("clipper", { input: 2, ceiling: -0.3, knee: 60, os: "8x" }),
    "Hard and loud": pkPre("clipper", { input: 12, ceiling: -0.5, knee: 0, os: "4x" }),
  },
  saturate: {
    "Warm tape": pkPre("saturate", { mode: "Tape", drive: 8, tone: -1.5, mix: 100 }),
    "Tube vocal": pkPre("saturate", { mode: "Tube", drive: 10, bias: 30, tone: 1, mix: 70 }),
    "Bass growl": pkPre("saturate", { mode: "Transistor", drive: 18, mix: 60 }),
    "Fuzz lead": pkPre("saturate", { mode: "Fuzz", drive: 26, tone: 2, mix: 100 }),
    "Octave up": pkPre("saturate", { mode: "Rectify", drive: 6, mix: 50 }),
  },
  multi: {
    "OTT classic": pkPre("multi", { depth: 100 }),
    "Gentle glue": pkPre("multi", { depth: 30, upward: 40, downward: 70 }),
    "Vocal air": pkPre("multi", { depth: 45, lowgain: -3, highgain: 3 }),
    "Squash": pkPre("multi", { depth: 100, time: 0.5, input: 4 }),
  },
  deess: { "Vocal": pkPre("deess", { freq: 6500, thresh: -30, range: 8 }), "Harsh singer": pkPre("deess", { freq: 5000, thresh: -36, range: 12 }),
    "Gentle air": pkPre("deess", { freq: 9000, thresh: -26, range: 5 }) },
  transient: { "Punchy drums": pkPre("transient", { attack: 50, sustain: -20 }), "Tight snare": pkPre("transient", { attack: 30, sustain: -50 }),
    "Soften attack": pkPre("transient", { attack: -50, sustain: 0 }), "Room up": pkPre("transient", { attack: 0, sustain: 60 }) },
  width: { "Wide master": pkPre("width", { width: 120, lowwidth: 80, highwidth: 140, monobass: 120 }), "Mono bass": pkPre("width", { monobass: 150 }),
    "Super wide": pkPre("width", { width: 170, monobass: 200 }), "Mono check": pkPre("width", { width: 0 }) },
  gate: { "Drum tight": pkPre("gate", { thresh: -30, range: 80, attack: 0.3, hold: 30, release: 80 }), "Vocal clean": pkPre("gate", { thresh: -48, range: 18, release: 250 }),
    "Kick (low only)": pkPre("gate", { thresh: -28, sclp: 200, release: 120 }) },
  filter: { "Slow sweep": pkPre("filter", { type: "Low pass", cutoff: 800, reso: 40, rate: "4 bars", lfo: 3 }), "Wobble": pkPre("filter", { type: "Low pass", slope: "24 dB", cutoff: 300, reso: 60, rate: "1/8", lfo: 3, drive: 6 }),
    "Auto-wah": pkPre("filter", { type: "Band pass", cutoff: 500, reso: 55, env: 3 }), "Radio": pkPre("filter", { type: "Band pass", cutoff: 1500, reso: 30 }),
    "Build-up riser": pkPre("filter", { type: "High pass", cutoff: 60, reso: 30, rate: "4 bars", shape: "Saw", lfo: 4 }) },
  lofi: { "Cassette": pkPre("lofi", { bits: 16, rate: 48000, wow: 35, flutter: 20, noise: 20, tone: 9000 }), "Sampler 12-bit": pkPre("lofi", { bits: 12, rate: 26000, wow: 0, flutter: 0, noise: 5, tone: 12000 }),
    "Crushed": pkPre("lofi", { bits: 5, rate: 6000, wow: 0, flutter: 0, noise: 0, tone: 20000 }), "Old vinyl": pkPre("lofi", { bits: 16, rate: 48000, wow: 15, flutter: 5, noise: 60, tone: 7000 }) },
  doubler: { "Vocal double": pkPre("doubler", { detune: 10, delay: 18, width: 80, voices: "2", mix: 45 }), "Huge chorus": pkPre("doubler", { detune: 22, delay: 25, width: 100, voices: "6", mix: 60 }),
    "Subtle width": pkPre("doubler", { detune: 6, delay: 10, width: 60, voices: "2", mix: 30, lowcut: 200 }) },
  autotune: { "Natural": pkPre("autotune", { speed: 60, humanize: 50, amount: 85 }), "Pop": pkPre("autotune", { speed: 20, humanize: 30, amount: 100 }),
    "Hard (robot)": pkPre("autotune", { speed: 0, humanize: 0, amount: 100 }), "Subtle": pkPre("autotune", { speed: 140, humanize: 65, amount: 60 }),
    "Octave down": pkPre("autotune", { scale: "Chromatic", amount: 0, transpose: -12 }) },
  utility: { "Mono": pkPre("utility", { mono: 1 }), "-6 dB": pkPre("utility", { gain: -6 }), "Flip phase": pkPre("utility", { phasel: 1, phaser: 1 }) },
  shaper: {
    "Quarter pump": pkPre("shaper", { shape: "Pump", rate: "1/4", depth: 100, curve: 50, hold: 5 }),
    "Lows only pump": pkPre("shaper", { shape: "Pump", rate: "1/4", depth: 100, band: "Lows", xover: 150 }),
    "Sidechain duck": pkPre("shaper", { trig: "Sidechain", shape: "Pump", rate: "1/8", depth: 90, thresh: -24 }),
    "Eighth chop": pkPre("shaper", { shape: "Square", rate: "1/8", depth: 100 }),
    "Slow swell": pkPre("shaper", { shape: "Saw up", rate: "1 bar", depth: 80 }),
    "Trance gate": pkPre("shaper", { shape: "Gate", rate: "1/4", depth: 100, smooth: 2 }),
  },
});

// --- the EQ Pro's filters, as plugins.rs makes them ---
function rbjJs(kind, f, q, g, sr) {
  const w = 2 * Math.PI * Math.max(5, Math.min(f, sr * 0.49)) / sr, c = Math.cos(w), sn = Math.sin(w), al = sn / (2 * q), A = 10 ** (g / 40);
  const n = (b, a) => [b[0] / a[0], b[1] / a[0], b[2] / a[0], a[1] / a[0], a[2] / a[0]];
  switch (kind) {
    case "lp": return n([(1 - c) / 2, 1 - c, (1 - c) / 2], [1 + al, -2 * c, 1 - al]);
    case "hp": return n([(1 + c) / 2, -(1 + c), (1 + c) / 2], [1 + al, -2 * c, 1 - al]);
    case "bp": return n([al, 0, -al], [1 + al, -2 * c, 1 - al]);
    case "notch": return n([1, -2 * c, 1], [1 + al, -2 * c, 1 - al]);
    case "peak": return n([1 + al * A, -2 * c, 1 - al * A], [1 + al / A, -2 * c, 1 - al / A]);
    default: {
      const sq = 2 * Math.sqrt(A) * al;
      return kind === "hs"
        ? n([A * ((A + 1) + (A - 1) * c + sq), -2 * A * ((A - 1) + (A + 1) * c), A * ((A + 1) + (A - 1) * c - sq)], [(A + 1) - (A - 1) * c + sq, 2 * ((A - 1) - (A + 1) * c), (A + 1) - (A - 1) * c - sq])
        : n([A * ((A + 1) - (A - 1) * c + sq), 2 * A * ((A - 1) - (A + 1) * c), A * ((A + 1) - (A - 1) * c - sq)], [(A + 1) + (A - 1) * c + sq, -2 * ((A - 1) + (A + 1) * c), (A + 1) + (A - 1) * c - sq]);
    }
  }
}
function eqpBand(slot, i) {
  const b = i + 1, v = (k) => slot[`b${b}${k}`] ?? PK_DEFAULTS.eqpro[`b${b}${k}`], sp = (k) => pkSpec("eqpro", `b${b}${k}`);
  return { i, on: v("on") >= 0.5, type: pkReal(sp("type"), v("type")), freq: pkReal(sp("freq"), v("freq")), gain: pkReal(sp("gain"), v("gain")),
    q: pkReal(sp("q"), v("q")), slope: [12, 24, 48][pkReal(sp("slope"), v("slope"))], ch: pkReal(sp("ch"), v("ch")),
    dyn: pkReal(sp("dyn"), v("dyn")), thr: pkReal(sp("thr"), v("thr")) };
}
function eqpStages(b, gain, sr) {
  const { freq: f, q } = b;
  if (b.type === 0) return [rbjJs("peak", f, q, gain, sr)];
  if (b.type === 1) return [rbjJs("ls", f, Math.min(q, 4), gain, sr)];
  if (b.type === 2) return [rbjJs("hs", f, Math.min(q, 4), gain, sr)];
  if (b.type === 5) return [rbjJs("notch", f, q, 0, sr)];
  if (b.type === 6) return [rbjJs("bp", f, q, 0, sr)];
  const qs = b.slope === 12 ? [0.7071] : b.slope === 24 ? [0.5412, 1.3066] : [0.5098, 0.6013, 0.9, 2.5629];
  return qs.map((bq, i) => rbjJs(b.type === 3 ? "hp" : "lp", f, i === qs.length - 1 ? bq * q / 0.7071 : bq, 0, sr));
}
const EQP_COLORS = ["#ff6b6b", "#ffa94d", "#ffd43b", "#69db7c", "#38d9a9", "#4dabf7", "#9775fa", "#f783ac"];

Object.assign(PK_DISPLAY, {
  // EQ Pro: the spectrum before (grey) and after (colour), every band's curve and the sum, a handle
  // per band. Drag a handle (Shift: frequency only), scroll on it for Q, double-click the curve to add
  // a band, double-click a handle to remove it, right-click it for its type, slope and channel.
  eqpro(slot, color, setParam, gesture, t, pos, st) {
    const sr = () => ui.rate || 48000, RANGE = 24;
    let spec = null, dynNow = null, drag = null, hover = null, sel = st?.band ?? 0;
    const geo = (w, h) => ({ x: (f) => Math.log(f / 20) / Math.log(1000) * w, fx: (px) => 20 * 1000 ** (px / w),
      y: (db) => h / 2 - db / RANGE * (h / 2 - 12), dby: (py) => (h / 2 - py) / (h / 2 - 12) * RANGE, sy: (db) => h - (Math.max(-96, Math.min(0, db)) + 96) / 96 * h });
    const flat = (b) => b.type >= 3; // cuts, notch, band pass sit on the zero line
    const node = (b, w, h) => { const G = geo(w, h); return [Math.max(9, Math.min(w - 9, G.x(b.freq))), flat(b) ? G.y(0) : G.y(b.gain)]; };
    const bands = () => Array.from({ length: 8 }, (_, i) => eqpBand(slot, i));
    const cv = pkCanvas("eqpd", (ctx, w, h) => {
      const G = geo(w, h), S = sr(), bs = bands();
      ctx.fillStyle = "rgba(255,255,255,0.045)";
      for (const f of [30, 50, 100, 200, 300, 500, 1000, 2000, 3000, 5000, 10000, 20000]) ctx.fillRect(Math.round(G.x(f)), 0, 1, h);
      for (const db of [-18, -12, -6, 6, 12, 18]) ctx.fillRect(0, Math.round(G.y(db)), w, 1);
      ctx.fillStyle = "rgba(255,255,255,0.12)"; ctx.fillRect(0, Math.round(G.y(0)), w, 1);
      ctx.fillStyle = "rgba(255,255,255,0.32)"; ctx.font = "600 9px system-ui, sans-serif";
      for (const [f, l] of [[50, "50"], [100, "100"], [200, "200"], [500, "500"], [1000, "1k"], [2000, "2k"], [5000, "5k"], [10000, "10k"]]) ctx.fillText(l, G.x(f) + 3, h - 4);
      for (const db of [-18, -12, -6, 6, 12, 18]) ctx.fillText((db > 0 ? "+" : "") + db, 3, G.y(db) - 2);
      // the spectrum: before (filled grey), after (a line in the colour)
      if (spec) {
        const path = (off) => { ctx.beginPath(); for (let i = 0; i < 64; i++) { const x = G.x(20 * 1000 ** ((i + 0.5) / 64)), y = G.sy(spec[off + i]); i ? ctx.lineTo(x, y) : ctx.moveTo(x, y); } };
        path(0); ctx.lineTo(w, h); ctx.lineTo(0, h); ctx.closePath(); ctx.fillStyle = "rgba(200,205,230,0.10)"; ctx.fill();
        path(64); ctx.strokeStyle = color + "66"; ctx.lineWidth = 1.2; ctx.stroke();
      }
      // each band's own curve, then the sum
      const on = bs.filter((b) => b.on);
      const stagesOf = on.map((b) => [b, eqpStages(b, b.gain, S)]);
      for (const [b, st] of stagesOf) {
        ctx.beginPath();
        for (let px = 0; px <= w; px += 2) { const db = Math.max(-RANGE - 6, Math.min(RANGE + 6, eqDb(st, G.fx(px), S))); px ? ctx.lineTo(px, G.y(db)) : ctx.moveTo(px, G.y(db)); }
        ctx.lineTo(w, G.y(0)); ctx.lineTo(0, G.y(0)); ctx.closePath();
        ctx.fillStyle = EQP_COLORS[b.i] + (b.i === sel ? "30" : "16"); ctx.fill();
      }
      const sumAt = (f) => stagesOf.reduce((a, [b, st]) => a + (b.ch === 0 || b.ch === 1 ? eqDb(st, f, S) : 0), 0);
      ctx.beginPath();
      for (let px = 0; px <= w; px++) { const db = Math.max(-RANGE - 6, Math.min(RANGE + 6, sumAt(G.fx(px)))); px ? ctx.lineTo(px, G.y(db)) : ctx.moveTo(px, G.y(db)); }
      glow(ctx, color, 12); ctx.strokeStyle = "#fff"; ctx.lineWidth = 2; ctx.stroke(); noGlow(ctx);
      // the dynamic bands where they are now
      for (const b of on) {
        if (Math.abs(b.dyn) < 0.05 || flat(b) || !dynNow) continue;
        const [x, y0] = node(b, w, h), y1 = G.y(dynNow[b.i]);
        ctx.strokeStyle = EQP_COLORS[b.i]; ctx.lineWidth = 3; ctx.globalAlpha = 0.7;
        ctx.beginPath(); ctx.moveTo(x, y0); ctx.lineTo(x, y1); ctx.stroke(); ctx.globalAlpha = 1;
        ctx.beginPath(); ctx.arc(x, y1, 4, 0, 2 * Math.PI); ctx.fillStyle = EQP_COLORS[b.i]; ctx.fill();
      }
      for (const b of on) {
        const [x, y] = node(b, w, h), hot = hover === b.i || drag?.i === b.i;
        ctx.beginPath(); ctx.arc(x, y, hot || b.i === sel ? 9 : 7.5, 0, 2 * Math.PI);
        ctx.fillStyle = EQP_COLORS[b.i]; glow(ctx, EQP_COLORS[b.i], hot ? 16 : 8); ctx.fill(); noGlow(ctx);
        if (b.i === sel) { ctx.strokeStyle = "#fff"; ctx.lineWidth = 2; ctx.stroke(); }
        ctx.fillStyle = "#0b0a0f"; ctx.font = "800 9.5px system-ui, sans-serif"; ctx.textAlign = "center";
        ctx.fillText(b.i + 1, x, y + 3.5); ctx.textAlign = "left";
        if (b.ch) { ctx.fillStyle = EQP_COLORS[b.i]; ctx.font = "800 8px system-ui"; ctx.fillText(["", "M", "S", "L", "R"][b.ch], x + 9, y - 7); }
      }
      const hb = drag ? bs[drag.i] : hover != null ? bs[hover] : null;
      if (hb) {
        const [x, y] = node(hb, w, h);
        const txt = `${EQP_TYPES[hb.type]}  ${pkFmt(pkSpec("eqpro", "b1freq"), pkNorm(pkSpec("eqpro", "b1freq"), hb.freq))}` +
          (flat(hb) ? (hb.type < 5 ? `  ${hb.slope} dB/oct` : "") : `  ${(hb.gain > 0 ? "+" : "") + hb.gain.toFixed(1)} dB`) + `  Q ${hb.q.toFixed(2)}`;
        ctx.font = "700 10.5px system-ui, sans-serif";
        const tw = ctx.measureText(txt).width + 12, tx = Math.min(w - tw - 2, Math.max(2, x - tw / 2)), ty = y < 34 ? y + 16 : y - 30;
        ctx.fillStyle = "rgba(10,9,14,0.9)"; ctx.fillRect(tx, ty, tw, 18);
        ctx.fillStyle = "#fff"; ctx.fillText(txt, tx + 6, ty + 13);
      }
    }, 240);
    cv.scope = (sc) => { if (!sc || sc.length < 136) return; spec = sc.slice(0, 128); dynNow = sc.slice(128, 136); cv.redraw(); };
    const at = (e) => { const r = cv.getBoundingClientRect(); return { px: e.clientX - r.left, py: e.clientY - r.top, w: r.width, h: r.height }; };
    const hit = (e) => {
      const q = at(e); let best = null, bd = 13;
      for (const b of bands()) { if (!b.on) continue; const [x, y] = node(b, q.w, q.h), d = Math.hypot(x - q.px, y - q.py); if (d < bd) { bd = d; best = b.i; } }
      return best;
    };
    const K = (i, k) => `b${i + 1}${k}`;
    const choose = (i) => { sel = i; cv.select?.(i); };
    cv.addEventListener("pointermove", (e) => { if (drag) return; const hb = hit(e); if (hb !== hover) { hover = hb; cv.style.cursor = hb != null ? "grab" : "crosshair"; cv.redraw(); } });
    cv.addEventListener("pointerleave", () => { hover = null; cv.redraw(); });
    cv.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      const i = hit(e);
      if (i == null) return;
      choose(i);
      cv.setPointerCapture(e.pointerId);
      gesture();
      drag = { i };
      const b0 = eqpBand(slot, i);
      const move = (ev) => {
        const q = at(ev), G = geo(q.w, q.h);
        setParam(K(i, "freq"), pkNorm(pkSpec("eqpro", K(i, "freq")), G.fx(Math.max(0, Math.min(q.w, q.px)))));
        if (!ev.shiftKey && b0.type < 3) setParam(K(i, "gain"), pkNorm(pkSpec("eqpro", K(i, "gain")), Math.max(-24, Math.min(24, G.dby(q.py)))));
        cv.redraw();
      };
      const up = () => { drag = null; cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up); cv.redraw(); };
      cv.addEventListener("pointermove", move); cv.addEventListener("pointerup", up);
    });
    cv.addEventListener("dblclick", (e) => {
      const i = hit(e);
      gesture();
      if (i != null) { setParam(K(i, "on"), 0); return; }
      const free = bands().find((b) => !b.on);
      if (!free) { toast("All 8 bands are in use."); return; }
      const q = at(e), G = geo(q.w, q.h), f = G.fx(q.px);
      const type = f < 35 ? 3 : f > 15000 ? 4 : 0;
      setParam(K(free.i, "type"), type / 6);
      setParam(K(free.i, "freq"), pkNorm(pkSpec("eqpro", K(free.i, "freq")), f));
      setParam(K(free.i, "gain"), type ? 0.5 : pkNorm(pkSpec("eqpro", K(free.i, "gain")), Math.max(-24, Math.min(24, G.dby(q.py)))));
      setParam(K(free.i, "q"), pkNorm(pkSpec("eqpro", K(free.i, "q")), 0.7071));
      setParam(K(free.i, "on"), 1);
      choose(free.i);
    });
    cv.addEventListener("wheel", (e) => {
      const i = hit(e);
      if (i == null) return;
      e.preventDefault(); gesture();
      const k = K(i, "q");
      setParam(k, Math.max(0, Math.min(1, (slot[k] ?? PK_DEFAULTS.eqpro[k]) - Math.sign(e.deltaY) * 0.025)));
    }, { passive: false });
    cv.addEventListener("contextmenu", (e) => {
      const i = hit(e);
      if (i == null) return;
      e.preventDefault(); e.stopPropagation(); choose(i);
      const b = eqpBand(slot, i), set = (k, v) => () => { gesture(); setParam(K(i, k), v); };
      showCtx([
        ...EQP_TYPES.map((n, j) => [(b.type === j ? "✓ " : "   ") + n, "", set("type", j / 6)]),
        ...(b.type === 3 || b.type === 4 ? [12, 24, 48].map((sl, j) => [(b.slope === sl ? "✓ " : "   ") + sl + " dB/octave", "", set("slope", j / 2)]) : []),
        ...["Stereo", "Mid", "Side", "Left", "Right"].map((n, j) => [(b.ch === j ? "✓ " : "   ") + "Channel: " + n, "", set("ch", j / 4)]),
        ["Remove band " + (i + 1), "", set("on", 0)],
      ], e.clientX, e.clientY);
    });
    cv.title = "Double-click to add a band. Drag a handle (Shift: frequency only), scroll on it for Q, right-click for type, slope and channel, double-click to remove.";
    // the bands as buttons, above the knobs: pick one, Shift-click switches it on or off
    const wrap = document.createElement("div");
    wrap.className = "eqpwrap";
    const strip = document.createElement("div");
    strip.className = "eqpbands";
    const paint = () => strip.querySelectorAll("button").forEach((x, i) => { x.classList.toggle("on", eqpBand(slot, i).on); x.classList.toggle("sel", i === sel); });
    for (let i = 0; i < 8; i++) {
      const x = document.createElement("button");
      x.textContent = i + 1;
      x.style.setProperty("--b", EQP_COLORS[i]);
      x.title = "Band " + (i + 1) + " (Shift-click: on/off)";
      x.onclick = (e) => { if (e.shiftKey || !eqpBand(slot, i).on) { gesture(); setParam(K(i, "on"), eqpBand(slot, i).on ? 0 : 1); } choose(i); paint(); };
      strip.append(x);
    }
    wrap.append(cv, strip);
    wrap.redraw = () => { cv.redraw(); paint(); };
    wrap.scope = cv.scope;
    Object.defineProperty(cv, "select", { get: () => wrap.select });
    paint();
    return wrap;
  },

  // Clipper: what goes in against what comes out, and how much it cut off over time
  clipper(slot, color) {
    const hist = [];
    const cv = pkCanvas("clipd", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("clipper", k), slot[k] ?? PK_DEFAULTS.clipper[k]);
      const ceil = 10 ** (g("ceiling") / 20), knee = slot.knee ?? PK_DEFAULTS.clipper.knee, gin = 10 ** (g("input") / 20);
      const sz = h - 16, ox = 8, oy = 8, X = (v) => ox + v * sz, Y = (v) => oy + sz - v * sz;
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (let k = 1; k < 4; k++) { ctx.fillRect(X(k / 4), oy, 1, sz); ctx.fillRect(ox, Y(k / 4), sz, 1); }
      ctx.strokeStyle = "rgba(255,255,255,0.12)"; ctx.beginPath(); ctx.moveTo(X(0), Y(0)); ctx.lineTo(X(1), Y(1)); ctx.stroke();
      const t = ceil * (1 - Math.min(0.999, knee));
      const clipf = (x) => { const a = Math.abs(x); if (a <= t) return x; const y = knee <= 0.001 ? ceil : t + (ceil - t) * Math.tanh((a - t) / (ceil - t)); return Math.min(y, ceil); };
      ctx.beginPath();
      for (let i = 0; i <= 100; i++) { const x = i / 100; i ? ctx.lineTo(X(x), Y(clipf(x * gin))) : ctx.moveTo(X(x), Y(clipf(x * gin))); }
      glow(ctx, color, 10); ctx.strokeStyle = color; ctx.lineWidth = 2.2; ctx.stroke(); noGlow(ctx);
      ctx.fillStyle = color + "aa"; ctx.fillRect(ox, Math.round(Y(ceil)), sz, 1);
      const hx = ox + sz + 14, hw = w - hx - 6;
      if (hw > 30) {
        ctx.fillStyle = "rgba(255,255,255,0.04)"; ctx.fillRect(hx, oy, hw, sz);
        const HX = (k) => hx + hw - (hist.length - 1 - k) / 199 * hw, HY = (db) => oy + Math.min(sz, db / 12 * sz);
        ctx.beginPath(); ctx.moveTo(HX(0), oy);
        hist.forEach((v, k) => ctx.lineTo(HX(k), HY(v)));
        ctx.lineTo(hx + hw, oy); ctx.closePath(); ctx.fillStyle = color + "77"; ctx.fill();
        ctx.fillStyle = "rgba(255,255,255,0.45)"; ctx.font = "600 9px system-ui, sans-serif";
        ctx.fillText("Clipped", hx + 4, oy + sz - 4);
        ctx.fillText(`${(hist[hist.length - 1] || 0).toFixed(1)} dB`, hx + hw - 40, oy + 11);
      }
      ctx.fillStyle = "rgba(255,255,255,0.45)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText(`Ceiling ${g("ceiling").toFixed(1)} dB`, ox + 4, oy + 11);
    }, 170);
    cv.live = (m) => { hist.push(m[2]); if (hist.length > 200) hist.shift(); cv.redraw(); };
    return cv;
  },

  // Saturator: the curve the sound is bent along, for the mode, drive and asymmetry
  saturate(slot, color) {
    let lvl = 0;
    const shape = (mode, x) => {
      if (mode === 0) return (2 / Math.PI) * Math.atan(Math.PI / 2 * x);
      if (mode === 1) { const t = Math.tanh(x); return t + 0.18 * t * t; }
      if (mode === 2) return x / Math.pow(1 + x ** 4, 0.25);
      if (mode === 3) return x > 0 ? Math.tanh(3 * x) : 0.8 * Math.tanh(2.2 * x);
      const t = Math.tanh(x); return 0.5 * t + 0.5 * Math.abs(t);
    };
    const cv = pkCanvas("satd", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("saturate", k), slot[k] ?? PK_DEFAULTS.saturate[k]);
      const mode = g("mode"), drive = 10 ** (g("drive") / 20), bias = 0.6 * (slot.bias ?? 0), off = shape(mode, bias);
      const sz = h - 16, ox = 16, oy = 8, X = (v) => ox + (v + 1) / 2 * sz, Y = (v) => oy + (1 - (Math.max(-1.3, Math.min(1.3, v)) + 1) / 2) * sz;
      ctx.fillStyle = "rgba(255,255,255,0.05)"; ctx.fillRect(X(0), oy, 1, sz); ctx.fillRect(ox, Y(0), sz, 1);
      ctx.strokeStyle = "rgba(255,255,255,0.12)"; ctx.beginPath(); ctx.moveTo(X(-1), Y(-1)); ctx.lineTo(X(1), Y(1)); ctx.stroke();
      ctx.beginPath();
      for (let i = 0; i <= 200; i++) { const x = i / 100 - 1, y = shape(mode, x * drive + bias) - off; i ? ctx.lineTo(X(x), Y(y)) : ctx.moveTo(X(x), Y(y)); }
      glow(ctx, color, 12); ctx.strokeStyle = color; ctx.lineWidth = 2.4; ctx.stroke(); noGlow(ctx);
      if (lvl > 1e-4) { const x = Math.min(1, lvl); ctx.beginPath(); ctx.arc(X(x), Y(shape(mode, x * drive + bias) - off), 5, 0, 2 * Math.PI); ctx.fillStyle = "#fff"; glow(ctx, "#fff", 12); ctx.fill(); noGlow(ctx); }
      // the harmonics it adds: a quick look at a sine through the curve
      const hx = ox + sz + 24, hw = w - hx - 10;
      if (hw > 60) {
        const N = 256, sig = Array.from({ length: N }, (_, n) => shape(mode, 0.7 * Math.sin(2 * Math.PI * n / N) * drive + bias) - off);
        const harm = Array.from({ length: 10 }, (_, k) => { let re = 0, im = 0; for (let n = 0; n < N; n++) { re += sig[n] * Math.cos(2 * Math.PI * (k + 1) * n / N); im += sig[n] * Math.sin(2 * Math.PI * (k + 1) * n / N); } return Math.hypot(re, im) * 2 / N; });
        const top = Math.max(...harm, 1e-6), bw = hw / 10;
        harm.forEach((a, k) => {
          const db = 20 * Math.log10(a / top + 1e-9), bh = Math.max(0, (db + 60) / 60) * (sz - 14);
          ctx.fillStyle = k === 0 ? "rgba(255,255,255,0.5)" : color; glow(ctx, color, k ? 6 : 0);
          ctx.fillRect(hx + k * bw + 2, oy + sz - bh, bw - 4, bh); noGlow(ctx);
        });
        ctx.fillStyle = "rgba(255,255,255,0.45)"; ctx.font = "600 9px system-ui, sans-serif";
        ctx.fillText("Harmonics", hx, oy + 9);
      }
    }, 150);
    cv.live = (m) => { lvl = m[0]; cv.redraw(); };
    return cv;
  },

  // Multiband: a column per band with its level, the two thresholds (drag them) and what it does
  multi(slot, color, setParam, gesture) {
    let sc = null;
    const names = ["low", "mid", "high"];
    const cv = pkCanvas("multd", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("multi", k), slot[k] ?? PK_DEFAULTS.multi[k]);
      const cw = (w - 16) / 3, top = 18, bot = h - 18, Y = (db) => top + (-Math.max(-72, Math.min(0, db))) / 72 * (bot - top);
      names.forEach((n, b) => {
        const x0 = 8 + b * cw + 6, ww = cw - 12;
        ctx.fillStyle = "rgba(255,255,255,0.035)"; ctx.fillRect(x0, top, ww, bot - top);
        const td = g(n + "down"), tu = g(n + "up");
        // the band's zones: pressed down above, lifted below
        ctx.fillStyle = "rgba(255,110,90,0.10)"; ctx.fillRect(x0, top, ww, Y(td) - top);
        ctx.fillStyle = "rgba(90,180,255,0.10)"; ctx.fillRect(x0, Y(tu), ww, bot - Y(tu));
        if (sc) {
          const lv = sc[2 * b], gn = sc[2 * b + 1];
          const grad = ctx.createLinearGradient(0, bot, 0, top); grad.addColorStop(0, color + "55"); grad.addColorStop(1, color);
          ctx.fillStyle = grad; glow(ctx, color, 8);
          ctx.fillRect(x0 + ww * 0.3, Y(lv), ww * 0.4, bot - Y(lv)); noGlow(ctx);
          ctx.fillStyle = gn >= 0 ? "#6cc4ff" : "#ff7a6b"; ctx.font = "800 11px system-ui, sans-serif"; ctx.textAlign = "center";
          ctx.fillText((gn >= 0 ? "+" : "") + gn.toFixed(1) + " dB", x0 + ww / 2, bot + 13);
          ctx.textAlign = "left";
        }
        for (const [t, c] of [[td, "#ff7a6b"], [tu, "#6cc4ff"]]) { ctx.fillStyle = c; ctx.fillRect(x0, Math.round(Y(t)) - 1, ww, 2); }
        ctx.fillStyle = "rgba(255,255,255,0.55)"; ctx.font = "800 9px system-ui, sans-serif"; ctx.textAlign = "center";
        ctx.fillText(n.toUpperCase(), x0 + ww / 2, top - 6); ctx.textAlign = "left";
      });
      ctx.fillStyle = "rgba(255,255,255,0.4)"; ctx.font = "600 9px system-ui, sans-serif"; ctx.textAlign = "center";
      ctx.fillText(pkFmt(pkSpec("multi", "xlow"), slot.xlow ?? PK_DEFAULTS.multi.xlow), 8 + cw, top - 6);
      ctx.fillText(pkFmt(pkSpec("multi", "xhigh"), slot.xhigh ?? PK_DEFAULTS.multi.xhigh), 8 + 2 * cw, top - 6);
      ctx.textAlign = "left";
    }, 190);
    cv.scope = (s) => { sc = s; cv.redraw(); };
    // drag a threshold line up or down
    cv.addEventListener("pointerdown", (e) => {
      const r = cv.getBoundingClientRect(), cw = (r.width - 16) / 3, b = Math.max(0, Math.min(2, Math.floor((e.clientX - r.left - 8) / cw)));
      const top = 18, bot = r.height - 18, dbAt = (py) => -Math.max(0, Math.min(72, (py - top) / (bot - top) * 72));
      const py = e.clientY - r.top, n = names[b];
      const td = pkReal(pkSpec("multi", n + "down"), slot[n + "down"] ?? 0.5), tu = pkReal(pkSpec("multi", n + "up"), slot[n + "up"] ?? 0.333);
      const Y = (db) => top + (-db) / 72 * (bot - top);
      const key = Math.abs(Y(td) - py) < Math.abs(Y(tu) - py) ? n + "down" : n + "up";
      if (Math.min(Math.abs(Y(td) - py), Math.abs(Y(tu) - py)) > 12) return;
      cv.setPointerCapture(e.pointerId); gesture();
      const move = (ev) => setParam(key, pkNorm(pkSpec("multi", key), Math.max(-60, dbAt(ev.clientY - r.top))));
      const up = () => { cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up); };
      cv.addEventListener("pointermove", move); cv.addEventListener("pointerup", up);
    });
    cv.title = "Red line: above it the band is pressed down. Blue line: below it the band is lifted up. Drag them.";
    return cv;
  },

  // Shaper: the shape over one period, with where it is now
  shaper(slot, color) {
    let sc = null;
    const curveAt = (shape, ph, depth, curve, hold) => {
      ph = ((ph % 1) + 1) % 1;
      let down;
      if (shape === 0) { if (ph < hold) down = 1; else { const u = Math.min(1, (ph - hold) / Math.max(1e-3, 1 - hold)); down = 1 - u ** (0.25 * 16 ** curve); } }
      else if (shape === 1) down = 0.5 + 0.5 * Math.cos(2 * Math.PI * ph);
      else if (shape === 2) down = ph < 0.5 ? 1 : 0;
      else if (shape === 3) down = 1 - ph;
      else if (shape === 4) down = ph;
      else down = Math.floor(ph * 4) % 2 === 1 ? 1 : 0;
      return 1 - depth * down;
    };
    const cv = pkCanvas("shpd", (ctx, w, h) => {
      const v = (k) => slot[k] ?? PK_DEFAULTS.shaper[k];
      const shape = pkReal(pkSpec("shaper", "shape"), v("shape")), depth = v("depth"), curve = v("curve"), hold = 0.5 * v("hold");
      const top = 12, bot = h - 14, Y = (g) => bot - g * (bot - top), X = (ph) => 8 + ph * (w - 16) / 2;
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (let k = 0; k <= 8; k++) ctx.fillRect(Math.round(X(k / 4)), top, 1, bot - top);
      ctx.beginPath(); ctx.moveTo(X(0), bot);
      for (let i = 0; i <= 400; i++) { const ph = i / 200; ctx.lineTo(X(ph), Y(curveAt(shape, ph, depth, curve, hold))); }
      ctx.lineTo(X(2), bot); ctx.closePath();
      const grad = ctx.createLinearGradient(0, top, 0, bot); grad.addColorStop(0, color + "66"); grad.addColorStop(1, color + "08");
      ctx.fillStyle = grad; ctx.fill();
      ctx.beginPath();
      for (let i = 0; i <= 400; i++) { const ph = i / 200; i ? ctx.lineTo(X(ph), Y(curveAt(shape, ph, depth, curve, hold))) : ctx.moveTo(X(ph), Y(curveAt(shape, ph, depth, curve, hold))); }
      glow(ctx, color, 10); ctx.strokeStyle = color; ctx.lineWidth = 2.2; ctx.stroke(); noGlow(ctx);
      if (sc) {
        const x = X(sc[0]);
        ctx.fillStyle = "rgba(255,255,255,0.6)"; ctx.fillRect(x, top, 1.5, bot - top);
        ctx.beginPath(); ctx.arc(x, Y(sc[1]), 5, 0, 2 * Math.PI); ctx.fillStyle = "#fff"; glow(ctx, "#fff", 12); ctx.fill(); noGlow(ctx);
      }
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
      const rate = pkFmt(pkSpec("shaper", "rate"), v("rate")), trig = pkReal(pkSpec("shaper", "trig"), v("trig"));
      ctx.fillText(`${rate}${trig ? " · on each hit" : " · in time"}`, 10, top + 9);
    }, 150);
    cv.scope = (s) => { sc = s; cv.redraw(); };
    return cv;
  },
});

Object.assign(PK_DISPLAY, {
  // De-esser: where it listens (from the frequency up), how loud that is against the threshold,
  // and what it took off over time
  deess(slot, color) {
    let lvl = -120;
    const hist = [];
    const cv = pkCanvas("dsd", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("deess", k), slot[k] ?? PK_DEFAULTS.deess[k]);
      const f = g("freq"), thr = g("thresh");
      const X = (hz) => 8 + Math.log(hz / 500) / Math.log(40) * (w * 0.55 - 16);
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (const hz of [1000, 2000, 5000, 10000]) ctx.fillRect(Math.round(X(hz)), 8, 1, h - 16);
      const grad = ctx.createLinearGradient(X(f) - 30, 0, X(f) + 10, 0); grad.addColorStop(0, color + "00"); grad.addColorStop(1, color + "55");
      ctx.fillStyle = grad; ctx.fillRect(X(f) - 30, 8, X(20000) - X(f) + 30, h - 16);
      ctx.fillStyle = color; ctx.fillRect(Math.round(X(f)), 8, 2, h - 16);
      ctx.fillStyle = "rgba(255,255,255,0.45)"; ctx.font = "600 9px system-ui, sans-serif";
      for (const [hz, l] of [[1000, "1k"], [2000, "2k"], [5000, "5k"], [10000, "10k"]]) ctx.fillText(l, X(hz) + 3, h - 6);
      ctx.fillText(pkFmt(pkSpec("deess", "freq"), slot.freq ?? PK_DEFAULTS.deess.freq), X(f) + 5, 18);
      // the level it hears against the threshold
      const mx = w * 0.58, mw = 14, Y = (db) => 8 + (-Math.max(-60, Math.min(0, db))) / 60 * (h - 16);
      ctx.fillStyle = "rgba(255,255,255,0.05)"; ctx.fillRect(mx, 8, mw, h - 16);
      ctx.fillStyle = lvl > thr ? "#ff7a6b" : color; glow(ctx, ctx.fillStyle, 8); ctx.fillRect(mx, Y(lvl), mw, h - 8 - Y(lvl)); noGlow(ctx);
      ctx.fillStyle = "#fff"; ctx.fillRect(mx - 3, Math.round(Y(thr)), mw + 6, 2);
      const hx = mx + mw + 14, hw = w - hx - 8;
      ctx.fillStyle = "rgba(255,255,255,0.04)"; ctx.fillRect(hx, 8, hw, h - 16);
      ctx.beginPath(); ctx.moveTo(hx + hw - (hist.length - 1) / 149 * hw, 8);
      hist.forEach((gr, k) => ctx.lineTo(hx + hw - (hist.length - 1 - k) / 149 * hw, 8 + Math.min(1, gr / 24) * (h - 16)));
      ctx.lineTo(hx + hw, 8); ctx.closePath(); ctx.fillStyle = "#ff7a6b66"; ctx.fill();
      ctx.fillStyle = "rgba(255,255,255,0.45)"; ctx.fillText("Taken off", hx + 4, h - 12);
    }, 140);
    cv.scope = (sc) => { if (sc?.length) lvl = sc[0]; };
    cv.live = (m) => { hist.push(m[2]); if (hist.length > 150) hist.shift(); cv.redraw(); };
    return cv;
  },

  // Transient: a hit before (grey) and after (colour)
  transient(slot, color) {
    let now = 0;
    const cv = pkCanvas("trd", (ctx, w, h) => {
      const att = (slot.attack ?? 0.5) * 2 - 1, sus = (slot.sustain ?? 0.5) * 2 - 1, mid = h / 2;
      const env = (t) => Math.min(1, t / 0.01) * Math.exp(-t * 6);
      const shaped = (t) => { const hit = Math.max(0, 1 - t / 0.06), tail = Math.min(1, Math.max(0, (t - 0.04) / 0.15)); return env(t) * 10 ** ((att * hit * 9 + sus * tail * 7) / 20); };
      for (const [fn, c, lw] of [[env, "rgba(255,255,255,0.25)", 1.2], [shaped, color, 2]]) {
        ctx.beginPath();
        for (let x = 0; x <= w; x++) { const t = x / w * 0.6, a = fn(t) * (mid - 10) * Math.sin(x * 0.9); x ? ctx.lineTo(x, mid - a * 0.9) : ctx.moveTo(x, mid); }
        ctx.strokeStyle = c; ctx.lineWidth = lw; if (lw > 1.5) glow(ctx, color, 8); ctx.stroke(); noGlow(ctx);
      }
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif"; ctx.textAlign = "right";
      ctx.fillText(`now ${(now > 0 ? "+" : "") + now.toFixed(1)} dB`, w - 8, 14); ctx.textAlign = "left";
    }, 130);
    cv.scope = (sc) => { if (sc?.length) { now = sc[0]; cv.redraw(); } };
    return cv;
  },

  // Imager: the stereo picture (a vectorscope), correlation and balance
  width(slot, color) {
    let sc = null;
    const trail = [];
    const cv = pkCanvas("wdd", (ctx, w, h) => {
      const sz = h - 16, cx = 8 + sz / 2, cy = 8 + sz / 2, R = sz / 2 - 4;
      ctx.strokeStyle = "rgba(255,255,255,0.08)"; ctx.lineWidth = 1;
      ctx.beginPath(); ctx.arc(cx, cy, R, 0, 2 * Math.PI); ctx.stroke();
      ctx.beginPath(); ctx.moveTo(cx, cy - R); ctx.lineTo(cx, cy + R); ctx.moveTo(cx - R, cy); ctx.lineTo(cx + R, cy);
      ctx.moveTo(cx - R * 0.7, cy - R * 0.7); ctx.lineTo(cx + R * 0.7, cy + R * 0.7); ctx.moveTo(cx + R * 0.7, cy - R * 0.7); ctx.lineTo(cx - R * 0.7, cy + R * 0.7); ctx.stroke();
      ctx.fillStyle = "rgba(255,255,255,0.4)"; ctx.font = "700 8.5px system-ui, sans-serif";
      ctx.fillText("M", cx + 3, cy - R + 9); ctx.fillText("L", cx - R * 0.7 - 8, cy - R * 0.7); ctx.fillText("R", cx + R * 0.7 + 3, cy - R * 0.7);
      // the dots: mid up, side across
      trail.forEach((pts, age) => {
        ctx.fillStyle = color; ctx.globalAlpha = (age + 1) / trail.length * 0.8;
        for (let i = 0; i < pts.length; i += 2) {
          const l = pts[i], r = pts[i + 1], m = (l + r) * 0.7, s = (r - l) * 0.7;
          ctx.fillRect(cx + Math.max(-R, Math.min(R, s * R * 2)) - 1, cy - Math.max(-R, Math.min(R, m * R * 2)) - 1, 2.2, 2.2);
        }
      });
      ctx.globalAlpha = 1;
      const bx = 8 + sz + 22, bw = w - bx - 12;
      if (sc && bw > 60) {
        const bar = (y, label, v, lo, hi, fmt) => {
          ctx.fillStyle = "rgba(255,255,255,0.06)"; ctx.fillRect(bx, y, bw, 8);
          const x = bx + (v - lo) / (hi - lo) * bw, zero = bx + (0 - lo) / (hi - lo) * bw;
          ctx.fillStyle = v < 0 && label === "Correlation" ? "#ff7a6b" : color;
          ctx.fillRect(Math.min(x, zero), y, Math.abs(x - zero) || 2, 8);
          ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
          ctx.fillText(`${label} ${fmt(v)}`, bx, y - 4);
        };
        bar(26, "Correlation", sc[0], -1, 1, (v) => (v > 0 ? "+" : "") + v.toFixed(2));
        bar(62, "Balance", sc[1], -1, 1, (v) => (Math.abs(v) < 0.02 ? "centre" : v < 0 ? "left " + Math.round(-v * 100) + "%" : "right " + Math.round(v * 100) + "%"));
        const g = (k) => pkReal(pkSpec("width", k), slot[k] ?? PK_DEFAULTS.width[k]);
        ctx.fillStyle = "rgba(255,255,255,0.5)";
        ctx.fillText(`Width ${Math.round(g("width"))}% · lows ${Math.round(g("lowwidth"))}% · highs ${Math.round(g("highwidth"))}%`, bx, 100);
        ctx.fillText(`Mono below ${pkFmt(pkSpec("width", "monobass"), slot.monobass ?? 0)}`, bx, 116);
      }
    }, 160);
    cv.scope = (s) => { if (!s || s.length < 66) return; sc = s; trail.push(s.slice(2, 66)); if (trail.length > 6) trail.shift(); cv.redraw(); };
    return cv;
  },

  // Gate: the level over time, the threshold, and when it's open
  gate(slot, color) {
    const lv = [], open = [];
    const cv = pkCanvas("gtd", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("gate", k), slot[k] ?? PK_DEFAULTS.gate[k]);
      const thr = g("thresh"), hy = g("hyst"), Y = (db) => 8 + (-Math.max(-80, Math.min(0, db))) / 80 * (h - 16);
      const X = (k) => w - (lv.length - 1 - k) / 199 * w;
      open.forEach((o, k) => { if (o > 0.02) { ctx.fillStyle = `rgba(154,208,79,${0.18 * o})`; ctx.fillRect(X(k) - w / 199, 8, w / 199 + 1, h - 16); } });
      ctx.beginPath(); ctx.moveTo(X(0), h - 8);
      lv.forEach((v, k) => ctx.lineTo(X(k), Y(v)));
      ctx.lineTo(w, h - 8); ctx.closePath(); ctx.fillStyle = color + "40"; ctx.fill();
      ctx.fillStyle = color; ctx.fillRect(0, Math.round(Y(thr)), w, 2);
      ctx.fillStyle = color + "66"; ctx.fillRect(0, Math.round(Y(thr - hy)), w, 1);
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText(`Opens at ${thr.toFixed(0)} dB, closes below ${(thr - hy).toFixed(0)} dB`, 8, 16);
    }, 140);
    cv.scope = (sc) => { if (!sc || sc.length < 2) return; lv.push(sc[0]); open.push(sc[1]); if (lv.length > 200) { lv.shift(); open.shift(); } cv.redraw(); };
    return cv;
  },

  // Filter: its response, where the cutoff is now (it moves with the LFO and the envelope)
  filter(slot, color) {
    let fNow = null;
    const cv = pkCanvas("fld", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("filter", k), slot[k] ?? PK_DEFAULTS.filter[k]);
      const type = g("type"), stages = g("slope") ? 2 : 1, kk = 2 - 1.95 * (slot.reso ?? PK_DEFAULTS.filter.reso), q = 1 / kk;
      const sr = ui.rate || 48000, X = (f) => Math.log(f / 20) / Math.log(1000) * w, Fx = (px) => 20 * 1000 ** (px / w), Y = (db) => h * 0.42 - db / 30 * (h * 0.42 - 8);
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (const f of [100, 1000, 10000]) ctx.fillRect(Math.round(X(f)), 0, 1, h);
      ctx.fillRect(0, Math.round(Y(0)), w, 1);
      const draw = (fc, col, lw, fill) => {
        const st = [];
        const last = rbjJs(["lp", "hp", "bp", "notch"][type], fc, q, 0, sr);
        for (let s = 0; s < stages; s++) st.push(s + 1 === stages ? last : rbjJs(["lp", "hp", "bp", "notch"][type], fc, 0.7071, 0, sr));
        ctx.beginPath();
        for (let px = 0; px <= w; px += 2) { const db = Math.max(-60, Math.min(30, eqDb(st, Fx(px), sr))); px ? ctx.lineTo(px, Y(db)) : ctx.moveTo(px, Y(db)); }
        if (fill) { ctx.lineTo(w, h); ctx.lineTo(0, h); ctx.closePath(); const gr = ctx.createLinearGradient(0, 0, 0, h); gr.addColorStop(0, color + "55"); gr.addColorStop(1, color + "05"); ctx.fillStyle = gr; ctx.fill(); }
        else { ctx.strokeStyle = col; ctx.lineWidth = lw; glow(ctx, col, 10); ctx.stroke(); noGlow(ctx); }
      };
      const base = g("cutoff");
      draw(base, "rgba(255,255,255,0.35)", 1.2, false);
      const fc = fNow ?? base;
      draw(fc, color, 2.2, true);
      draw(fc, color, 2.2, false);
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText(`${["Low pass", "High pass", "Band pass", "Notch"][type]} · ${pkFmt(pkSpec("filter", "cutoff"), pkNorm(pkSpec("filter", "cutoff"), fc))}`, 8, 14);
    }, 150);
    cv.scope = (sc) => { if (sc?.length) { fNow = sc[0]; cv.redraw(); } };
    return cv;
  },

  // Lo-fi: a wave as it comes out (steps from the bits and the rate, wobble from wow)
  lofi(slot, color) {
    const t0 = performance.now();
    const cv = pkCanvas("lfd", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("lofi", k), slot[k] ?? PK_DEFAULTS.lofi[k]);
      const bits = g("bits"), rate = g("rate"), wow = slot.wow ?? 0, mid = h / 2, q = 2 ** (bits - 1);
      const hold = Math.max(1, Math.round(48000 / rate / 40)), tt = (performance.now() - t0) / 1000;
      ctx.beginPath();
      for (let x = 0; x <= w; x++) { const y = Math.sin(x / w * 4 * Math.PI) * 0.8; x ? ctx.lineTo(x, mid - y * (mid - 10)) : ctx.moveTo(x, mid - y * (mid - 10)); }
      ctx.strokeStyle = "rgba(255,255,255,0.18)"; ctx.lineWidth = 1; ctx.stroke();
      ctx.beginPath();
      let held = 0;
      for (let x = 0; x <= w; x++) {
        if (x % hold === 0) { const ph = x / w * 4 * Math.PI + wow * 0.6 * Math.sin(tt * 3.4 + x / w * 3); held = Math.round(Math.sin(ph) * 0.8 * q) / q; }
        const y = mid - held * (mid - 10);
        x ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
      }
      glow(ctx, color, 8); ctx.strokeStyle = color; ctx.lineWidth = 2; ctx.stroke(); noGlow(ctx);
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText(`${bits.toFixed(1)} bit · ${pkFmt(pkSpec("lofi", "rate"), slot.rate ?? PK_DEFAULTS.lofi.rate)}`, 8, 14);
    }, 130);
    cv.live = () => cv.redraw();
    return cv;
  },

  // Doubler: the copies, where they sit (left to right) and how far they're tuned
  doubler(slot, color) {
    return pkCanvas("dbd", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("doubler", k), slot[k] ?? PK_DEFAULTS.doubler[k]);
      const voices = 1 + g("voices"), cents = g("detune"), width = slot.width ?? 0.8, mix = slot.mix ?? 0.5;
      const cx = w / 2, cy = h / 2;
      ctx.fillStyle = "rgba(255,255,255,0.05)"; ctx.fillRect(cx, 8, 1, h - 16); ctx.fillRect(8, cy, w - 16, 1);
      ctx.fillStyle = "rgba(255,255,255,0.4)"; ctx.font = "700 8.5px system-ui, sans-serif";
      ctx.fillText("L", 10, cy - 4); ctx.fillText("R", w - 16, cy - 4); ctx.fillText("+" + Math.round(cents) + " ct", cx + 4, 16); ctx.fillText("−" + Math.round(cents) + " ct", cx + 4, h - 10);
      ctx.beginPath(); ctx.arc(cx, cy, 7, 0, 2 * Math.PI); ctx.fillStyle = "#fff"; ctx.fill();
      for (let v = 0; v < 2 * voices; v++) {
        const up = v % 2 === 0, k = (Math.floor(v / 2) + 1) / voices;
        const pan = (up ? -1 : 1) * width * (0.5 + 0.5 * k), det = (up ? 1 : -1) * k;
        const x = cx + pan * (w / 2 - 24), y = cy - det * (h / 2 - 22);
        ctx.beginPath(); ctx.arc(x, y, 4 + 6 * mix, 0, 2 * Math.PI);
        ctx.fillStyle = color; glow(ctx, color, 10); ctx.globalAlpha = 0.5 + 0.5 * mix; ctx.fill(); noGlow(ctx); ctx.globalAlpha = 1;
      }
    }, 130);
  },
});

Object.assign(PK_DISPLAY, {
  // Tune live: the pitch it hears (thin) and what comes out (bright), on the notes of the scale,
  // and the song's key one click away
  autotune(slot, color, setParam, gesture) {
    const heard = [], out = [];
    let target = -1, centre = 60;
    const NAMES = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];
    const SC = [[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], [0, 2, 4, 5, 7, 9, 11], [0, 2, 3, 5, 7, 8, 10], [0, 2, 3, 5, 7, 8, 11], [0, 2, 4, 7, 9], [0, 3, 5, 7, 10], [0, 3, 5, 6, 7, 10]];
    const cv = pkCanvas("atd", (ctx, w, h) => {
      const key = pkReal(pkSpec("autotune", "key"), slot.key ?? 0), scale = pkReal(pkSpec("autotune", "scale"), slot.scale ?? 1 / 6);
      const span = 14, lo = centre - span / 2, Y = (m) => h - 8 - (m - lo) / span * (h - 16), gx = 34;
      for (let n = Math.floor(lo); n <= Math.ceil(lo + span); n++) {
        const inScale = SC[scale].includes(((n - key) % 12 + 12) % 12), y = Y(n);
        ctx.fillStyle = n === target ? color + "40" : inScale ? "rgba(255,255,255,0.07)" : "rgba(255,255,255,0.02)";
        ctx.fillRect(gx, Math.round(y) - (n === target ? 1 : 0), w - gx, n === target ? 3 : 1);
        if (inScale) { ctx.fillStyle = n === target ? color : "rgba(255,255,255,0.45)"; ctx.font = "700 9px system-ui, sans-serif"; ctx.fillText(NAMES[((n % 12) + 12) % 12] + (Math.floor(n / 12) - 1), 4, y + 3); }
      }
      const X = (k) => gx + (w - gx - 4) - (heard.length - 1 - k) / 239 * (w - gx - 4);
      for (const [arr, col, lw] of [[heard, "rgba(255,255,255,0.45)", 1.2], [out, color, 2.4]]) {
        ctx.beginPath(); let pen = false;
        arr.forEach((m, k) => { if (m < 0) { pen = false; return; } const y = Y(m); pen ? ctx.lineTo(X(k), y) : ctx.moveTo(X(k), y); pen = true; });
        ctx.strokeStyle = col; ctx.lineWidth = lw; if (lw > 2) glow(ctx, color, 10); ctx.stroke(); noGlow(ctx);
      }
    }, 190);
    cv.scope = (sc) => {
      if (!sc || sc.length < 4) return;
      heard.push(sc[0]); out.push(sc[1]); target = sc[2];
      if (heard.length > 240) { heard.shift(); out.shift(); }
      if (sc[0] > 0) centre += (sc[0] - centre) * 0.05;
      cv.redraw();
    };
    // the song's key
    const wrap = document.createElement("div");
    wrap.className = "eqpwrap";
    const bar = document.createElement("div");
    bar.className = "atbar";
    const paint = () => {
      const k = typeof songKey === "function" ? songKey() : null;
      bar.innerHTML = k ? `<span>Song key: <b>${esc(k)}</b></span><button class="pkb">Use it</button>` : `<span class="fine">Set the song's key in the chord tool to follow it here.</span>`;
      const b = bar.querySelector("button");
      if (b) b.onclick = () => {
        const [root, mode] = k.split(" "), i = NAMES.indexOf(root);
        if (i < 0) return;
        gesture();
        setParam("key", i / 11);
        setParam("scale", (mode === "minor" ? 2 : 1) / 6);
        cv.redraw();
      };
    };
    paint();
    wrap.append(cv, bar);
    wrap.redraw = () => cv.redraw();
    wrap.scope = cv.scope;
    return wrap;
  },
});

// ---------- a compact list to pick from, that you can search by typing ----------
// entries: [{ group, name, sub, run }]; it scrolls when it's taller than the window
function pkPicker(entries, x, y, placeholder = "Search…") {
  document.querySelector(".pkpick")?.close();
  hideCtx?.();
  const el = document.createElement("div");
  el.className = "pkpick";
  el.innerHTML = `<input class="pkq" placeholder="${esc(placeholder)}" spellcheck="false" autocomplete="off"><div class="pkl2"></div>`;
  const q = el.querySelector(".pkq"), list = el.querySelector(".pkl2");
  let shown = [], act = 0;
  const render = () => {
    const words = q.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
    shown = entries.filter((e) => words.every((w) => `${e.name} ${e.sub || ""} ${e.group || ""}`.toLowerCase().includes(w)));
    act = Math.min(act, Math.max(0, shown.length - 1));
    list.replaceChildren();
    let group = null;
    shown.forEach((e, i) => {
      if (e.group !== group) {
        group = e.group;
        if (group) { const h = document.createElement("div"); h.className = "pkgh"; h.textContent = group; list.append(h); }
      }
      const b = document.createElement("button");
      b.className = "pkit" + (i === act ? " act" : "");
      b.innerHTML = `${e.color ? `<i style="background:${e.color}"></i>` : ""}<span>${esc(e.name)}</span>${e.sub ? `<em>${esc(e.sub)}</em>` : ""}`;
      b.onmousemove = () => { if (act !== i) { act = i; paint(); } };
      b.onclick = () => choose(i);
      list.append(b);
    });
    if (!shown.length) list.innerHTML = `<div class="pkgh none">Nothing called "${esc(q.value)}"</div>`;
  };
  const paint = () => list.querySelectorAll(".pkit").forEach((b, i) => b.classList.toggle("act", i === act));
  const choose = (i) => { const e = shown[i]; if (!e) return; el.close(); e.run(); };
  q.addEventListener("input", () => { act = 0; render(); });
  q.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      act = (act + (e.key === "ArrowDown" ? 1 : -1) + shown.length) % Math.max(1, shown.length);
      paint();
      list.querySelectorAll(".pkit")[act]?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") { e.preventDefault(); choose(act); }
    else if (e.key === "Escape") { e.preventDefault(); el.close(); }
  });
  const away = (e) => { if (!el.contains(e.target)) el.close(); };
  el.close = () => { el.remove(); document.removeEventListener("pointerdown", away, true); };
  document.body.append(el);
  render();
  // in the window: below the click if it fits, else above; never taller than the window
  const r = el.getBoundingClientRect();
  el.style.left = Math.max(8, Math.min(x, innerWidth - r.width - 8)) + "px";
  el.style.top = (y + r.height > innerHeight - 8 ? Math.max(8, innerHeight - r.height - 8) : y) + "px";
  setTimeout(() => document.addEventListener("pointerdown", away, true));
  q.focus();
  return el;
}
