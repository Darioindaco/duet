"use strict";
// Duet: MIDI-learn. Right-click a knob or fader of a song channel (or the master): "Link to a MIDI
// controller", then move a knob, fader or the pitch wheel on the MIDI keyboard; from then on it turns
// that control. Links are this machine's (it's your keyboard), kept per channel and control:
// { cc: [{ target: track id or "master", param, label }] } (cc 128: the pitch wheel).
// Loaded after auto.js; autoOnControl() registers each control it sees.

const ml = { links: {}, els: new Map(), learning: null, seen: null, count: 0, timer: null };

function mlSetup() {
  ml.links = store.get("midiLinks", {}) || {};
  mlRun();
}

const mlKey = (target, param) => `${target}|${param}`;
const mlCcName = (cc) => (cc === 128 ? "the pitch wheel" : `CC ${cc}`);

function mlLinkOf(target, param) {
  for (const [cc, list] of Object.entries(ml.links)) if (list.some((l) => l.target === target && l.param === param)) return +cc;
  return null;
}

function mlSave() { store.set("midiLinks", ml.links); mlRun(); }

function mlUnlink(target, param) {
  for (const cc of Object.keys(ml.links)) {
    ml.links[cc] = ml.links[cc].filter((l) => !(l.target === target && l.param === param));
    if (!ml.links[cc].length) delete ml.links[cc];
  }
  mlSave();
}

// wait for the next thing moved on the keyboard
function mlLearn(target, param, label) {
  ml.learning = { target, param, label, from: null };
  toast(`Move a knob or fader on your MIDI keyboard to link it to ${label} (Esc: cancel)`);
  mlRun();
}

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && ml.learning) { ml.learning = null; toast("Not linked"); mlRun(); }
});

// the strip a target plays on now ("T<slot>" or "M")
function mlStrip(target) {
  if (target === "master") return "M";
  const t = song.view?.tracks.find((k) => k.id === target);
  return t && t.slot != null ? "T" + t.slot : null;
}

// a control on screen: moving it does everything its own mouse would (the song, the engine, undo)
function mlRegister(el, strip, param) {
  ml.els.set(`${strip}|${param}`, el);
}

// a control that isn't on screen: straight into the mixer and the engine
function mlDirect(strip, param, x) {
  if (param === "vol") { setStrip(strip, "fader", faderGain(x)); return; }
  if (param === "pan") { setStrip(strip, "pan", x * 2 - 1); return; }
  const cfg = stripCfg(strip);
  if (!cfg) return;
  const fx = param.match(/^fx:(\d+):(\w+):(\w+)$/);
  if (fx) {
    const s = (cfg.rack || [])[+fx[1]];
    if (!s || s.kind !== fx[2]) return;
    s[fx[3]] = x;
    api.rack_set(strip, +fx[1], fx[3], x);
    saveMixer();
    return;
  }
  const inst = param.match(/^inst:(\w+)$/);
  if (inst && cfg.inst) {
    cfg.inst[inst[1]] = x;
    api.inst_param(strip, inst[1], x);
    saveMixer();
  }
}

function mlApply(link, x) {
  const strip = mlStrip(link.target);
  if (!strip) return;
  const el = ml.els.get(`${strip}|${link.param}`);
  if (el && el.isConnected && el.midiSet) el.midiSet(x);
  else mlDirect(strip, link.param, x);
}

// poll the keyboard's controllers while there's a link or a learn going on
function mlRun() {
  const need = ml.learning || Object.keys(ml.links).length;
  if (need && !ml.timer) ml.timer = setInterval(mlTick, 30);
  if (!need && ml.timer) { clearInterval(ml.timer); ml.timer = null; ml.seen = null; }
}

async function mlTick() {
  if (mlTick.busy || !api.midi_controllers) return;
  mlTick.busy = true;
  try {
    const c = await api.midi_controllers();
    if (!c) return;
    const L = ml.learning;
    if (L) {
      if (L.from == null) L.from = c.count;
      else if (c.count > L.from) {
        const cc = c.last;
        for (const k of Object.keys(ml.links)) ml.links[k] = ml.links[k].filter((l) => !(l.target === L.target && l.param === L.param));
        (ml.links[cc] ||= []).push({ target: L.target, param: L.param, label: L.label });
        ml.learning = null;
        ml.seen = c.values.slice();
        mlSave();
        toast(`${L.label} is linked to ${mlCcName(cc)}`);
        return;
      }
    }
    // what moved since the last look turns what it's linked to (not on the first look: no jumps)
    if (ml.seen && song.view) {
      for (const [cc, list] of Object.entries(ml.links)) {
        const v = c.values[+cc];
        if (v >= 0 && v !== ml.seen[+cc]) for (const l of list) mlApply(l, v);
      }
    }
    ml.seen = c.values.slice();
  } catch (e) {
    // (the next look tries again)
  } finally {
    mlTick.busy = false;
  }
}

// the menu items for a control's right-click menu
function mlMenu(target, param, label) {
  const cc = mlLinkOf(target, param);
  return cc == null
    ? [[`Link ${label} to a MIDI controller…`, "", () => mlLearn(target, param, label)]]
    : [[`Linked to ${mlCcName(cc)}: link again…`, "", () => mlLearn(target, param, label)],
       [`Unlink from ${mlCcName(cc)}`, "", () => { mlUnlink(target, param); toast(`${label} isn't linked any more`); }]];
}
