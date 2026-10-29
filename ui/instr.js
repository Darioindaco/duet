"use strict";
// Duet: instruments. A strip of yours can play an instrument instead of an
// input: the built-in drum kit, the built-in synth or a VST3 instrument. You
// play it with a MIDI keyboard or the computer keyboard; your partner hears it
// like any other channel, and a song track can record it.
//
// Loaded before the main script; everything here runs after boot().

const INST = {
  drums: { name: "Drum kit", sub: "built-in · 16 pads", color: "#ff7a59",
    knobs: [["level", "Level", (v) => Math.round(v * 100) + "%"], ["tune", "Tune", (v) => { const s = Math.round((v - 0.5) * 12); return (s > 0 ? "+" : "") + s + " st"; }],
      ["decay", "Decay", (v) => (0.05 + v * 4).toFixed(1) + " s"]] },
  synth: { name: "Synth", sub: "built-in · 8 voices", color: "#b58cff",
    knobs: [["wave", "Wave", (v) => (v < 0.15 ? "saw" : v > 0.85 ? "square" : Math.round(v * 100) + "%")],
      ["cutoff", "Cutoff", (v) => { const f = 60 * 2 ** (v * 8.4); return f >= 1000 ? (f / 1000).toFixed(1) + "k" : Math.round(f) + ""; }],
      ["resonance", "Reso", (v) => Math.round(v * 100) + "%"], ["attack", "Attack", (v) => fmtSecs(0.001 * 2000 ** v)],
      ["release", "Release", (v) => fmtSecs(0.01 * 400 ** v)], ["detune", "Detune", (v) => Math.round(v * 12) + " ct"]] },
  vst: { name: "VST instrument", sub: "VST3", color: "#9fb4ff", knobs: [] },
  sampler: { name: "Sampler", sub: "one sound, every pitch", color: "#ffb347",
    knobs: [["level", "Level", (v) => Math.round(v * 100) + "%"], ["tune", "Tune", (v) => { const s = Math.round((v - 0.5) * 24); return (s > 0 ? "+" : "") + s + " st"; }],
      ["attack", "Attack", (v) => fmtSecs(0.0005 + 0.5 * v * v)], ["release", "Release", (v) => fmtSecs(0.005 + 3 * v * v)],
      ["root", "Root", (v) => noteName(60 + Math.round((v - 0.5) * 72))], ["oneshot", "One-shot", (v) => (v >= 0.5 ? "on" : "off")]] },
};
const INST_DEFAULTS = { drums: { level: 0.8, tune: 0.5, decay: 0.5 }, synth: { wave: 0.3, cutoff: 0.55, resonance: 0.25, attack: 0.05, release: 0.3, detune: 0.2 },
  sampler: { level: 0.8, tune: 0.5, attack: 0, release: 0.15, root: 0.5, oneshot: 0 } };
function fmtSecs(s) { return s < 1 ? Math.round(s * 1000) + " ms" : s.toFixed(1) + " s"; }

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
  if (inst.kind === "drums") body.append(padGrid(t, inst));
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

function padGrid(t, inst) {
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
