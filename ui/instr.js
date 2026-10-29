"use strict";
// Duet: instruments. A strip of yours can play an instrument instead of an
// input: the built-in drum kit, the built-in synth or a VST3 instrument. You
// play it with a MIDI keyboard or the computer keyboard; your partner hears it
// like any other channel, and a song track can record it.
//
// Loaded before the main script; everything here runs after boot().

const pctF = (v) => Math.round(v * 100) + "%";
const semiF = (range) => (v) => { const s = Math.round((v - 0.5) * range); return (s > 0 ? "+" : "") + s + " st"; };
const hzF = (f) => (f >= 1000 ? (f / 1000).toFixed(f >= 10000 ? 0 : 1) + "k" : f >= 10 ? Math.round(f) + "" : f.toFixed(1)) + " Hz";
const timesF = (span) => (v) => "×" + (2 ** ((v - 0.5) * span)).toFixed(2);
const panF = (v) => { const p = Math.round((v - 0.5) * 200); return p === 0 ? "C" : p < 0 ? -p + " L" : p + " R"; };
const shapeF = (v) => (v < 0.04 ? "saw" : v > 0.96 ? "square" : Math.round(v * 100) + "%");
// the times as the engine reads them (synth.rs)
const synAttack = (v) => 0.001 * 2000 ** v, synDecay = (v) => 0.005 * 800 ** v, synRelease = (v) => 0.02 * 500 ** v;

const DRUM_KITS = ["808", "909", "Punchy", "Lo-fi"];
const SYN_FILTERS = ["LP 24", "LP 12", "BP", "HP"];
const INST = {
  drums: { name: "Drum kit", sub: "built-in · synthesized · 16 pads", color: "#ff7a59", knobs: [],
    kit: [["level", "Level", pctF], ["tune", "Tune", semiF(12)], ["decay", "Decay", timesF(2)], ["drive", "Drive", pctF]],
    pad: [["tune", "Tune", semiF(24)], ["decay", "Decay", timesF(3)], ["tone", "Tone", pctF], ["level", "Level", pctF], ["pan", "Pan", panF]] },
  synth: { name: "Synth", sub: "built-in · 16 voices", color: "#b58cff", knobs: [],
    sections: [
      ["Oscillators", [["wave", "Osc 1", shapeF], ["osc2", "Osc 2", pctF], ["osc2_wave", "Shape 2", shapeF], ["osc2_semi", "Pitch 2", semiF(24)],
        ["sub", "Sub", pctF], ["noise", "Noise", pctF]]],
      ["Unison", [["unison", "Voices", (v) => 1 + Math.round(v * 6) + ""], ["detune", "Detune", (v, inst) => Math.round(v * (inst.unison > 1 / 12 ? 40 : 20)) + " ct"],
        ["spread", "Width", pctF]]],
      ["Filter", [["cutoff", "Cutoff", (v) => hzF(60 * 2 ** (v * 8.4))], ["resonance", "Reso", pctF],
        ["env_amt", "Env", (v) => { const o = (v - 0.5) * 12; return (o > 0.05 ? "+" : "") + o.toFixed(1) + " oct"; }],
        ["key_track", "Key", pctF]], "filter"],
      ["Filter envelope", [["f_attack", "A", (v) => fmtSecs(synAttack(v))], ["f_decay", "D", (v) => fmtSecs(synDecay(v))],
        ["f_sustain", "S", pctF], ["f_release", "R", (v) => fmtSecs(synRelease(v))]], "fenv"],
      ["Amp envelope", [["attack", "A", (v) => fmtSecs(synAttack(v))], ["decay", "D", (v) => fmtSecs(synDecay(v))],
        ["sustain", "S", pctF], ["release", "R", (v) => fmtSecs(synRelease(v))]], "aenv"],
      ["LFO", [["lfo_rate", "Rate", (v) => hzF(0.05 * 2 ** (v * 8))], ["lfo_pitch", "Vibrato", (v) => (v * v * 2).toFixed(2) + " st"],
        ["lfo_filter", "Filter", (v) => (v * 3).toFixed(1) + " oct"], ["lfo_amp", "Tremolo", pctF]]],
      ["Play", [["glide", "Glide", (v) => (v < 0.001 ? "off" : fmtSecs(v * v))], ["velocity", "Velocity", pctF], ["drive", "Drive", pctF],
        ["level", "Level", pctF]], "mono"],
    ] },
  vst: { name: "VST instrument", sub: "VST3", color: "#9fb4ff", knobs: [] },
  sampler: { name: "Sampler", sub: "one sound, every pitch", color: "#ffb347",
    knobs: [["level", "Level", pctF], ["tune", "Tune", semiF(24)],
      ["attack", "Attack", (v) => fmtSecs(0.0005 + 0.5 * v * v)], ["release", "Release", (v) => fmtSecs(0.005 + 3 * v * v)],
      ["root", "Root", (v) => noteName(60 + Math.round((v - 0.5) * 72))], ["oneshot", "One-shot", (v) => (v >= 0.5 ? "on" : "off")]] },
};
// the engine's defaults (drums.rs, synth.rs, instr.rs)
const SYNTH_DEFAULTS = { wave: 0.3, cutoff: 0.55, resonance: 0.25, attack: 0.05, release: 0.3, detune: 0.2, decay: 0.5, sustain: 1, osc2: 0.5,
  osc2_semi: 0.5, osc2_wave: 0.3, sub: 0, noise: 0, unison: 0, spread: 0.5, filter: 0, env_amt: 0.5, f_attack: 0, f_decay: 0.55, f_sustain: 0,
  f_release: 0.3, key_track: 0.3, lfo_rate: 0.45, lfo_pitch: 0, lfo_filter: 0, lfo_amp: 0, glide: 0, mono: 0, velocity: 0.6, drive: 0, level: 0.7 };
const DRUM_PAN = { 42: 0.62, 44: 0.62, 46: 0.62, 49: 0.64, 51: 0.36, 41: 0.36, 43: 0.42, 47: 0.56, 48: 0.6, 50: 0.64 };
const DRUM_DEFAULTS = { level: 0.8, tune: 0.5, decay: 0.5, kit: 0.6, drive: 0.2 };
Array.from({ length: 16 }, (_, i) => 36 + i).forEach((n, i) => Object.assign(DRUM_DEFAULTS, { [`p${i}_tune`]: 0.5, [`p${i}_decay`]: 0.5, [`p${i}_tone`]: 0.5, [`p${i}_level`]: 0.8, [`p${i}_pan`]: DRUM_PAN[n] ?? 0.5 }));
const INST_DEFAULTS = { drums: DRUM_DEFAULTS, synth: SYNTH_DEFAULTS,
  sampler: { level: 0.8, tune: 0.5, attack: 0, release: 0.15, root: 0.5, oneshot: 0 } };
function fmtSecs(s) { return s < 1 ? Math.round(s * 1000) + " ms" : s.toFixed(1) + " s"; }

// factory sounds for the synth: what each changes from the defaults
const SYNTH_PRESETS = {
  Init: {},
  "Warm pad": { wave: 0.2, attack: 0.84, decay: 0.8, sustain: 0.8, release: 0.69, unison: 0.5, detune: 0.35, spread: 0.85, cutoff: 0.5, resonance: 0.15,
    env_amt: 0.56, f_attack: 0.82, f_decay: 0.85, f_sustain: 0.4, lfo_rate: 0.4, lfo_filter: 0.12, level: 0.62 },
  "Supersaw lead": { wave: 0, osc2_wave: 0, unison: 1, detune: 0.45, spread: 0.9, cutoff: 0.74, resonance: 0.18, attack: 0.1, release: 0.48, glide: 0.12, level: 0.6 },
  "Sub bass": { wave: 0.5, osc2: 0, sub: 0.85, cutoff: 0.36, resonance: 0.1, env_amt: 0.62, f_decay: 0.62, attack: 0, release: 0.41, mono: 1, glide: 0.1, detune: 0.04, key_track: 0.5 },
  "Acid bass": { wave: 0, osc2: 0, detune: 0, cutoff: 0.28, resonance: 0.82, env_amt: 0.82, f_decay: 0.59, attack: 0, release: 0.37, mono: 1, glide: 0.22, drive: 0.5, key_track: 0.2, velocity: 0.8 },
  Pluck: { wave: 0.3, cutoff: 0.3, resonance: 0.3, env_amt: 0.86, f_decay: 0.62, decay: 0.66, sustain: 0, release: 0.52, attack: 0, unison: 0.17, detune: 0.22, velocity: 0.8 },
  Keys: { wave: 0.75, osc2: 0.3, osc2_semi: 1, osc2_wave: 0.5, cutoff: 0.45, env_amt: 0.7, f_decay: 0.75, f_sustain: 0.2, decay: 0.85, sustain: 0.3, release: 0.52,
    lfo_rate: 0.6, lfo_amp: 0.12, velocity: 0.9, detune: 0.1 },
  Brass: { wave: 0.1, attack: 0.54, cutoff: 0.38, env_amt: 0.72, f_attack: 0.6, f_decay: 0.69, f_sustain: 0.45, resonance: 0.2, unison: 0.17, detune: 0.25, release: 0.48, velocity: 0.8 },
  "Wobble bass": { wave: 0.2, mono: 1, sub: 0.6, cutoff: 0.3, resonance: 0.55, lfo_rate: 0.74, lfo_filter: 0.7, drive: 0.6, unison: 0.17, detune: 0.3, release: 0.37, attack: 0 },
  Strings: { wave: 0.05, osc2_wave: 0.05, attack: 0.77, release: 0.66, unison: 0.5, detune: 0.3, spread: 1, cutoff: 0.55, filter: 0.375, lfo_rate: 0.66, lfo_pitch: 0.12, level: 0.62 },
  "Chip lead": { wave: 1, osc2: 0, detune: 0, cutoff: 1, resonance: 0, attack: 0, release: 0.32, lfo_rate: 0.8, lfo_pitch: 0.2, filter: 0.375 },
  "Noise riser": { noise: 1, osc2: 0, filter: 0.625, cutoff: 0.3, resonance: 0.6, env_amt: 0.9, f_attack: 1, f_decay: 0.95, f_sustain: 0.6, attack: 0.96, release: 0.74, key_track: 0 },
};


// computer keyboard: one row of white keys (A S D F G H J K L ;), the black
// ones above them (W E  T Y U  O P), Z / X an octave down / up
const KEYMAP = { a: 0, w: 1, s: 2, e: 3, d: 4, f: 5, t: 6, g: 7, y: 8, h: 9, u: 10, j: 11, k: 12, o: 13, l: 14, p: 15, ";": 16 };
const typing = { on: false, octave: 4, held: new Map(), target: null, midiTarget: undefined };

const instLabel = (inst) => (inst.kind === "vst" ? inst.vst?.name || "Instrument" : INST[inst.kind].name);

// the strip the keyboards play: while recording, an armed channel; in the
// sequencer, the channel you're on; else the selected strip if it has an
// instrument, else your first strip that has one
function instTarget() {
  if (!ui.mixer) return null;
  const chan = (tid) => { const t = song.view?.tracks.find((x) => x.id === tid); return t && t.inst && t.slot != null ? "T" + t.slot : null; };
  if (song.st?.recording) {
    const armed = song.view?.tracks.find((t) => t.inst && t.arm && t.slot != null);
    if (armed) return "T" + armed.slot;
  }
  if (ui.view === "seq" && chan(seq.channel)) return chan(seq.channel);
  if (ui.sel[0] === "T" && stripCfg(ui.sel)?.inst) return ui.sel;
  const has = (t) => t[0] === "L" && stripCfg(t)?.active && stripCfg(t)?.inst;
  if (has(ui.sel)) return ui.sel;
  const i = ui.mixer.local.findIndex((s) => s.active && s.inst);
  return i < 0 ? null : "L" + i;
}

function syncMidiTarget() {
  const t = instTarget();
  if (t !== typing.midiTarget) {
    typing.midiTarget = t;
    api.midi_target(t);
    allNotesOff();
  }
  paintKeys();
}

function noteOn(key, vel = 100) {
  const t = instTarget();
  if (!t) return false;
  api.note(t, true, key, vel);
  return t;
}

function allNotesOff() {
  for (const [, [t, key]] of typing.held) api.note(t, false, key);
  typing.held.clear();
}

// ---------- the computer keyboard ----------
function typingKeys(e, down) {
  if (!typing.on || e.ctrlKey || e.metaKey || e.altKey) return;
  if (e.target.closest && e.target.closest("input, select, textarea")) return;
  const k = e.key.toLowerCase();
  if (k === "z" || k === "x") {
    e.preventDefault(); e.stopImmediatePropagation();
    if (down && !e.repeat) { allNotesOff(); typing.octave = Math.max(0, Math.min(8, typing.octave + (k === "x" ? 1 : -1))); store.set("octave", typing.octave); paintKeys(); }
    return;
  }
  if (!(k in KEYMAP)) return;
  e.preventDefault(); e.stopImmediatePropagation();
  if (down) {
    if (e.repeat || typing.held.has(k)) return;
    const key = 12 * (typing.octave + 1) + KEYMAP[k];
    const t = noteOn(key, 100);
    if (!t) { toast("Pick an instrument first: on one of your strips, choose Drum kit, Synth or a VST instrument as the input."); return; }
    typing.held.set(k, [t, key]);
  } else {
    const h = typing.held.get(k);
    if (h) { api.note(h[0], false, h[1]); typing.held.delete(k); }
  }
}

function paintKeys() {
  const b = $("keysBtn");
  if (!b) return;
  const t = typing.midiTarget;
  b.classList.toggle("on", typing.on);
  const oct = `C${typing.octave}`;
  $("keysTxt").textContent = typing.on ? `Keys · ${oct}` : "Keys";
  b.title = (typing.on ? `Your computer keyboard plays ${t ? stripName(t) : "nothing yet"} (A–; are the white keys, W E T Y U O P the black ones; Z / X: octave ${oct} down / up). Click to stop.`
    : "Play the selected instrument with your computer keyboard");
}

// ---------- MIDI keyboards (Sound tab) ----------
async function midiRefresh() {
  let list = [];
  try { list = await api.midi_inputs(); } catch {}
  const sel = $("midiDev");
  const want = store.get("midiDev", "");
  const key = list.join("\n");
  if (key !== midiRefresh.key) {
    midiRefresh.key = key;
    sel.innerHTML = "";
    sel.append(new Option(list.length ? "None" : "None connected", ""));
    for (const n of list) sel.append(new Option(n, n));
    sel.value = list.includes(want) ? want : "";
    // the keyboard you used last time is back (plugged in again): listen to it
    if (want && list.includes(want) && midiRefresh.open !== want) await midiOpen(want, true);
    if (midiRefresh.open && !list.includes(midiRefresh.open)) midiRefresh.open = null;
  }
}

async function midiOpen(name, quiet = false) {
  const res = await api.midi_open(name || null);
  if (res && res.error) { if (!quiet) toast(`Couldn't open ${name}: ${res.error}`, true); midiRefresh.open = null; return; }
  midiRefresh.open = name || null;
  if (name && !quiet) toast(`${name} plays ${instTarget() ? stripName(instTarget()) : "the instrument you pick"}.`);
}

let lastMidiEvents = 0;
function paintMidi(n) {
  // a short blink of the keys button for every note
  if (n == null || n === lastMidiEvents) return;
  lastMidiEvents = n;
  const b = $("keysBtn");
  if (!b) return;
  b.classList.add("hit");
  clearTimeout(paintMidi.t);
  paintMidi.t = setTimeout(() => b.classList.remove("hit"), 120);
}

function instSetup() {
  typing.on = store.get("typing", false);
  typing.octave = store.get("octave", 4);
  $("keysBtn").onclick = () => { typing.on = !typing.on; store.set("typing", typing.on); if (!typing.on) allNotesOff(); paintKeys(); };
  window.addEventListener("keydown", (e) => typingKeys(e, true), true);
  window.addEventListener("keyup", (e) => typingKeys(e, false), true);
  window.addEventListener("blur", allNotesOff);
  $("midiDev").onchange = () => { store.set("midiDev", $("midiDev").value); midiOpen($("midiDev").value); };
  midiRefresh();
  setInterval(midiRefresh, 3000);
  syncMidiTarget();
}

// ---------- a strip's source: an input or an instrument ----------
function fillStripSource(src, t, cfg) {
  for (let i = 0; i < Math.max(inputCount(), cfg.input + 1); i++) src.append(new Option(`In ${i + 1}`, "in:" + i, false, !cfg.inst && i === cfg.input));
  const g = document.createElement("optgroup");
  g.label = "Instruments";
  g.append(new Option("Drum kit", "inst:drums", false, cfg.inst?.kind === "drums"));
  g.append(new Option("Synth", "inst:synth", false, cfg.inst?.kind === "synth"));
  if (cfg.inst?.kind === "vst") g.append(new Option(instLabel(cfg.inst), "inst:vst-current", false, true));
  g.append(new Option("VST instrument…", "inst:vst"));
  src.append(g);
}

async function stripSourceChanged(t, value) {
  const cfg = stripCfg(t);
  if (value.startsWith("in:")) {
    if (cfg.inst) await rackChanged(await api.inst_set(t, null));
    await setStrip(t, "input", +value.slice(3));
    renderStrips(); renderRack(); syncMidiTarget(); sendNames();
    return;
  }
  if (value === "inst:vst-current") return;
  if (value === "inst:vst") { ui.instFor = t; renderStrips(); openVstBrowser("inst"); return; }
  const kind = value.slice(5);
  const res = await api.inst_set(t, kind);
  if (res && res.error) { toast(res.error, true); renderStrips(); return; }
  await instMonitor(t);
  await rackChanged(res);
  selectStrip(t);
  syncMidiTarget(); sendNames();
  if (!typing.on && !midiRefresh.open) toast("Play it with your MIDI keyboard (Sound tab), or turn on Keys (top right) to use your computer keyboard.");
}

// an instrument has no direct monitoring on your interface: hear it through Duet
async function instMonitor(t) {
  if (!stripCfg(t).mon) await setStrip(t, "mon", true);
}

// ---------- the instrument panel (first in the selected strip's rack) ----------
function instPanel(t, inst) {
  const meta = INST[inst.kind];
  const p = document.createElement("div");
  p.className = "pedal instp " + inst.kind;
  p.style.setProperty("--c", meta.color);
  const title = instLabel(inst);
  p.innerHTML = `<div class="top"><div class="ttl"><div class="name" title="${esc(title)}">${esc(title)}</div><div class="sub">${esc(inst.kind === "vst" ? "VST3 instrument" : meta.sub)}</div></div>
    <div class="tools"><button class="del" title="Back to an input" aria-label="Remove the instrument">✕</button></div><span class="led"></span></div>`;
  p.querySelector(".del").onclick = async () => { await rackChanged(await api.inst_set(t, null)); syncMidiTarget(); sendNames(); };
  const body = document.createElement("div");
  body.className = "instbody";
  p.append(body);
  if (inst.error) body.insertAdjacentHTML("beforeend", `<div class="mnote bad">${esc(inst.error)}</div>`);
  if (inst.kind === "drums") body.append(drumBody(t, inst));
  if (inst.kind === "synth") body.append(synthBody(t, inst));
  if (inst.kind === "sampler") body.append(samplerBox(t, inst));
  if (inst.kind === "vst") {
    const open = document.createElement("button");
    open.className = "openwin";
    open.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M3 9h18"/></svg>Open window`;
    open.disabled = !inst.vst?.has_editor;
    open.title = inst.vst?.has_editor ? "The instrument's own controls" : "This instrument has no window of its own";
    open.onclick = async () => { const r = await api.vst_open(t, -1); if (r && r.error) toast(r.error, true); };
    body.append(open);
  }
  if (meta.knobs.length) {
    const knobs = document.createElement("div");
    knobs.className = "knobs";
    for (const [key, label, fmt] of meta.knobs) {
      knobs.append(autoOnControl(knob({ label, value: inst[key] ?? INST_DEFAULTS[inst.kind][key], def: INST_DEFAULTS[inst.kind][key], fmt, color: meta.color,
        onChange: (v) => { inst[key] = v; api.inst_param(t, key, v); saveMixer(); } }), t, `inst:${key}`));
    }
    body.append(knobs);
  }
  const hint = document.createElement("div");
  hint.className = "insthint";
  hint.innerHTML = `Play it with ${midiRefresh.open ? `<b>${esc(midiRefresh.open)}</b> or ` : ""}<button class="mb">${typing.on ? "the computer keyboard (on)" : "the computer keyboard"}</button>`;
  hint.querySelector("button").onclick = () => { if (!typing.on) $("keysBtn").click(); selectStrip(t); };
  p.append(hint);
  return p;
}

// the sampler's sound: its name, a button to load one; drop one from the browser here
function samplerBox(t, inst) {
  const own = inst.pads?.[0];
  const box = document.createElement("div");
  box.className = "smpbox";
  box.dataset.t = t;
  box.innerHTML = `<b>${own ? esc(own.split(/[\\/]/).pop().replace(/\.(wav|wave|mp3|m4a|aac|mp4|flac|aif|aiff|aifc|ogg|oga)$/i, "").replace(/^s-[0-9a-f]+$/, "Sample")) : "No sound yet"}</b>
    <span>${own ? "Drag another one here from the browser" : "Drag one here from the browser, or"}</span><button class="mb">Load a sample…</button>`;
  box.querySelector("button").onclick = async () => {
    const path = await api.inst_pad_open();
    if (!path) return;
    const res = await api.inst_pad(t, 0, path);
    if (res && res.error) toast(res.error, true); else await rackChanged(res);
  };
  return box;
}

// one of an instrument's knobs; `then` runs after every change (to redraw what depends on it)
function instKnob(t, inst, key, label, fmt, then, size = 40) {
  const def = INST_DEFAULTS[inst.kind]?.[key] ?? 0;
  const k = knob({ label, value: inst[key] ?? def, def, fmt: (v) => fmt(v, inst), color: INST[inst.kind].color, size,
    onChange: (v) => { inst[key] = v; api.inst_param(t, key, v); saveMixer(); then?.(key, v); } });
  k.dataset.key = key;
  return autoOnControl(k, t, `inst:${key}`);
}

// several knobs at once (a preset, a reset): the engine, the song, then the panel again
async function instSetMany(t, inst, vals) {
  for (const [k, v] of Object.entries(vals)) {
    inst[k] = v;
    api.inst_param(t, k, v);
  }
  saveMixer();
  fxRefresh();
}

// ---------- the synth ----------
const userSynthPresets = () => store.get("synthPresets", {});

function envPath(a, d, s, r, w = 132, h = 34) {
  // the times on a square-root scale: short ones still show, long ones fit
  const k = w / 4.2, xa = Math.sqrt(a) * k, xd = Math.sqrt(d) * k * 0.8, hold = w * 0.16, xr = Math.sqrt(r) * k * 0.8;
  const sc = Math.min(1, w / (xa + xd + hold + xr));
  const X = [0, xa * sc, (xa + xd) * sc, (xa + xd) * sc + hold, (xa + xd + xr) * sc + hold];
  const y = (v) => (h - 3 - v * (h - 6)).toFixed(1);
  return `M0 ${y(0)}L${X[1].toFixed(1)} ${y(1)}Q${(X[1] + (X[2] - X[1]) * 0.25).toFixed(1)} ${y(s)} ${X[2].toFixed(1)} ${y(s)}L${X[3].toFixed(1)} ${y(s)}Q${(X[3] + (X[4] - X[3]) * 0.2).toFixed(1)} ${y(0)} ${X[4].toFixed(1)} ${y(0)}`;
}

function synthBody(t, inst) {
  const v = (k) => inst[k] ?? SYNTH_DEFAULTS[k];
  const wrap = document.createElement("div");
  wrap.className = "synb";
  // presets: the factory's and yours
  const bar = document.createElement("div");
  bar.className = "synbar";
  const sel = document.createElement("select");
  sel.className = "synpre";
  sel.title = "Presets: a starting point; every knob still moves";
  const fill = () => {
    const mine = userSynthPresets();
    sel.innerHTML = `<option value="">Presets…</option><optgroup label="Factory">${Object.keys(SYNTH_PRESETS).map((n) => `<option value="f:${esc(n)}">${esc(n)}</option>`).join("")}</optgroup>`
      + (Object.keys(mine).length ? `<optgroup label="Yours">${Object.keys(mine).map((n) => `<option value="u:${esc(n)}">${esc(n)}</option>`).join("")}</optgroup>` : "");
  };
  fill();
  sel.onchange = () => {
    const [src, name] = [sel.value.slice(0, 1), sel.value.slice(2)];
    if (!sel.value) return;
    const vals = src === "f" ? { ...SYNTH_DEFAULTS, ...SYNTH_PRESETS[name] } : { ...SYNTH_DEFAULTS, ...userSynthPresets()[name] };
    instSetMany(t, inst, vals);
  };
  const save = document.createElement("button");
  save.className = "mb";
  save.textContent = "Save sound…";
  save.title = "Keep these settings as a preset of your own (on this computer)";
  save.onclick = () => {
    const name = (prompt("A name for this sound:") || "").trim();
    if (!name) return;
    const mine = userSynthPresets();
    mine[name] = Object.fromEntries(Object.keys(SYNTH_DEFAULTS).map((k) => [k, v(k)]));
    store.set("synthPresets", mine);
    fill();
    toast(`Saved “${name}”`);
  };
  const del = document.createElement("button");
  del.className = "mb";
  del.textContent = "Delete";
  del.title = "Delete the preset of yours that's chosen";
  del.onclick = () => {
    if (!sel.value.startsWith("u:")) return toast("Choose one of your own presets first", true);
    const mine = userSynthPresets();
    delete mine[sel.value.slice(2)];
    store.set("synthPresets", mine);
    fill();
  };
  bar.append(sel, save, del);
  wrap.append(bar);
  const grid = document.createElement("div");
  grid.className = "synsecs";
  const envs = {};
  const redraw = (key) => {
    if (envs.fenv) envs.fenv.setAttribute("d", envPath(synAttack(v("f_attack")), synDecay(v("f_decay")), v("f_sustain"), synRelease(v("f_release"))));
    if (envs.aenv) envs.aenv.setAttribute("d", envPath(synAttack(v("attack")), synDecay(v("decay")), v("sustain"), synRelease(v("release"))));
    if (key === "unison") grid.querySelector('[data-key="detune"]')?.setValue(v("detune")); // its cents follow the voices
  };
  for (const [title, knobs, extra] of INST.synth.sections) {
    const sec = document.createElement("div");
    sec.className = "synsec" + (extra ? " " + extra : "");
    sec.innerHTML = `<div class="synt">${esc(title)}</div>`;
    if (extra === "filter") {
      // the filter's kind: four buttons
      const seg = document.createElement("div");
      seg.className = "seg";
      SYN_FILTERS.forEach((name, i) => {
        const b = document.createElement("button");
        b.textContent = name;
        b.classList.toggle("on", Math.min(3, Math.floor(v("filter") * 4)) === i);
        b.onclick = () => {
          inst.filter = (i + 0.5) / 4;
          api.inst_param(t, "filter", inst.filter);
          saveMixer();
          seg.querySelectorAll("button").forEach((x, j) => x.classList.toggle("on", j === i));
        };
        seg.append(b);
      });
      sec.append(seg);
    }
    if (extra === "fenv" || extra === "aenv") {
      sec.insertAdjacentHTML("beforeend", `<svg class="envsvg" width="132" height="34" viewBox="0 0 132 34"><path fill="none" stroke-width="2" stroke-linejoin="round"/></svg>`);
      envs[extra] = sec.querySelector("path");
    }
    const row = document.createElement("div");
    row.className = "knobs";
    for (const [key, label, fmt] of knobs) row.append(instKnob(t, inst, key, label, fmt, redraw));
    sec.append(row);
    if (extra === "mono") {
      const b = document.createElement("button");
      b.className = "mb mono" + (v("mono") >= 0.5 ? " on" : "");
      b.textContent = "Mono / legato";
      b.title = "One note at a time; a new key while one is held slides there (with Glide) without starting the envelopes again";
      b.onclick = () => {
        inst.mono = v("mono") >= 0.5 ? 0 : 1;
        api.inst_param(t, "mono", inst.mono);
        saveMixer();
        b.classList.toggle("on", inst.mono >= 0.5);
      };
      sec.append(b);
    }
    grid.append(sec);
  }
  wrap.append(grid);
  redraw();
  return wrap;
}

// ---------- the drum kit ----------
const drumSel = new Map(); // strip -> the pad whose knobs show

function drumBody(t, inst) {
  const v = (k) => inst[k] ?? DRUM_DEFAULTS[k];
  const wrap = document.createElement("div");
  wrap.className = "drumb";
  const side = document.createElement("div");
  side.className = "drumkit";
  const seg = document.createElement("div");
  seg.className = "seg kits";
  seg.title = "The kit: how every pad is made";
  DRUM_KITS.forEach((name, i) => {
    const b = document.createElement("button");
    b.textContent = name;
    b.classList.toggle("on", Math.min(3, Math.floor(v("kit") * 4)) === i);
    b.onclick = () => {
      inst.kit = (i + 0.5) / 4;
      api.inst_param(t, "kit", inst.kit);
      saveMixer();
      seg.querySelectorAll("button").forEach((x, j) => x.classList.toggle("on", j === i));
      api.note(t, true, 36, 110); // hear it
    };
    seg.append(b);
  });
  const kk = document.createElement("div");
  kk.className = "knobs";
  for (const [key, label, fmt] of INST.drums.kit) kk.append(instKnob(t, inst, key, label, fmt, null, 42));
  side.append(seg, kk);
  const padBox = document.createElement("div");
  padBox.className = "padside";
  const knobsFor = () => {
    const i = drumSel.get(t) ?? 0;
    const name = inst.pad_names?.[i]?.[1] || `Pad ${i + 1}`;
    padBox.innerHTML = `<div class="padt"><b>${esc(name)}</b><button class="mb" title="This pad back to how it was">Reset</button></div>`;
    const row = document.createElement("div");
    row.className = "knobs";
    for (const [key, label, fmt] of INST.drums.pad) row.append(instKnob(t, inst, `p${i}_${key}`, label, fmt, () => marks(), 42));
    padBox.append(row);
    if (inst.pads?.[i]) padBox.insertAdjacentHTML("beforeend", `<div class="mnote">Your own sample: tune and decay change it; tone is for the built-in sound.</div>`);
    padBox.querySelector("button").onclick = () => instSetMany(t, inst, Object.fromEntries(INST.drums.pad.map(([k]) => [`p${i}_${k}`, DRUM_DEFAULTS[`p${i}_${k}`]])));
  };
  const grid = padGrid(t, inst, (i) => { drumSel.set(t, i); knobsFor(); marks(); });
  // which pads are changed, which one's knobs show
  const marks = () => grid.querySelectorAll(".pad").forEach((p, i) => {
    p.classList.toggle("sel", i === (drumSel.get(t) ?? 0));
    p.classList.toggle("edited", INST.drums.pad.some(([k]) => Math.abs(v(`p${i}_${k}`) - DRUM_DEFAULTS[`p${i}_${k}`]) > 0.005));
  });
  knobsFor();
  marks();
  wrap.append(side, grid, padBox);
  return wrap;
}

function padGrid(t, inst, onPick) {
  const grid = document.createElement("div");
  grid.className = "pads";
  (inst.pad_names || []).forEach(([note, name], i) => {
    const own = inst.pads?.[i];
    const pad = document.createElement("button");
    pad.className = "pad" + (own ? " own" : "");
    pad.dataset.t = t; pad.dataset.pad = i; // (a sound from the browser can be dropped on it)
    pad.innerHTML = `<b>${esc(own ? own.split(/[\\/]/).pop().replace(/\.(wav|wave|mp3|m4a|aac|mp4|flac|aif|aiff|aifc|ogg|oga)$/i, "") : name)}</b><span>${note}</span><i class="pmenu" title="Your own sound for this pad">⋯</i>`;
    pad.title = (own ? `${name} (note ${note}): ${own}` : `${name} (note ${note})`) + ". The higher you click, the harder it hits.";
    pad.onpointerdown = (e) => {
      if (e.target.classList.contains("pmenu")) return;
      e.preventDefault();
      pad.setPointerCapture(e.pointerId);
      pad.classList.add("hit");
      onPick?.(i);
      // harder towards the top of the pad
      const r = pad.getBoundingClientRect();
      const vel = Math.round(60 + 67 * (1 - (e.clientY - r.top) / r.height));
      api.note(t, true, note, vel);
      const up = () => { pad.classList.remove("hit"); api.note(t, false, note); pad.removeEventListener("pointerup", up); pad.removeEventListener("pointercancel", up); };
      pad.addEventListener("pointerup", up);
      pad.addEventListener("pointercancel", up);
    };
    pad.querySelector(".pmenu").onclick = async (e) => {
      e.stopPropagation();
      const items = [["Load a sample…", "", async () => {
        const path = await api.inst_pad_open();
        if (!path) return;
        const res = await api.inst_pad(t, i, path);
        if (res && res.error) toast(res.error, true); else await rackChanged(res);
      }]];
      if (own) items.push(["Back to the built-in sound", "", async () => rackChanged(await api.inst_pad(t, i, null))]);
      const r = pad.getBoundingClientRect();
      showCtx(items, r.left, r.bottom + 4);
    };
    grid.append(pad);
  });
  return grid;
}
