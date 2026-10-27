"use strict";
// Duet: the song's mixer beyond plain strips. Buses (mixer tracks that other
// tracks feed: a shared reverb, a drum bus), where each track goes (a bus or
// the master) and its sends, a compressor's sidechain, the EQ's curve. All of
// it is part of the shared song, like a track's effects.
//
// Loaded before the main script; the main script's helpers exist by the time
// anything here runs.

const BUS_COLOR = "#c9a3ff";

const busesOf = () => (song.view ? song.view.buses || [] : []);
const allTracks = () => (song.view ? [...song.view.tracks, ...(song.view.buses || [])] : []);
const trackById = (id) => allTracks().find((t) => t.id === id);

// where a track's sound goes: its bus (if that's a bus) and its sends
function routesOf(t) {
  const out = [];
  if (t.out && trackById(t.out)?.bus) out.push(t.out);
  for (const [b, v] of Object.entries(t.sends || {})) if (v > 0 && trackById(b)?.bus) out.push(b);
  return out;
}

// does `from`'s sound end up in `to`? (a bus can't go to one that feeds it)
function reaches(from, to) {
  const seen = new Set(), stack = [from];
  while (stack.length) {
    const x = stack.pop();
    if (x === to) return true;
    if (seen.has(x)) continue;
    seen.add(x);
    const t = trackById(x);
    if (t) stack.push(...routesOf(t));
  }
  return false;
}

const feedersOf = (bus) => allTracks().filter((t) => t.id !== bus.id && routesOf(t).includes(bus.id));

function addBusMenu(e) {
  const r = e.currentTarget.getBoundingClientRect();
  showCtx([
    ["Reverb bus", "", () => addBus("reverb")],
    ["Delay bus", "", () => addBus("delay")],
    ["Empty bus (e.g. a drum bus)", "", () => addBus("")],
  ], r.left, r.bottom + 4);
}

function addBus(kind) {
  const v = song.view, buses = busesOf();
  const order = 1000 + buses.reduce((a, b) => Math.max(a, (b.order || 1000) - 1000), 0) + 1;
  const name = kind === "reverb" ? "Reverb" : kind === "delay" ? "Delay" : `Bus ${buses.length + 1}`;
  // a send bus is all effect: the dry sound is already there from the track itself
  const rack = kind === "reverb" ? [{ kind: "reverb", on: true, ...FX_DEFAULTS.reverb, size: 0.75, mix: 0.5, dry: 0 }]
    : kind === "delay" ? [{ kind: "delay", on: true, ...FX_DEFAULTS.delay, feedback: 0.45, mix: 1, pingpong: 0.6, dry: 0 }] : [];
  songEdit([["track", newId("b"), { name, order, owner: v.me, bus: true, vol: 0.8, pan: 0, rack }]]);
  toast(kind ? `${name} bus added: select a track in the mixer, then click the arrow under "${name}" to send it there.`
    : "Bus added: select a track, then click the arrow under the bus (right-click: route it there only).");
}

function deleteBus(b) {
  const n = feedersOf(b).length;
  if (n && !confirm(`Delete "${b.name}"? ${n} track${n === 1 ? "" : "s"} going to it go${n === 1 ? "es" : ""} to the master again.`)) return;
  const ch = [["track", b.id, { del: true }]];
  for (const t of allTracks()) {
    if (t.out === b.id) ch.push(["track", t.id, { out: null }]);
    if (t.sends && b.id in t.sends) {
      const s = { ...t.sends };
      delete s[b.id];
      ch.push(["track", t.id, { sends: s }]);
    }
  }
  songEdit(ch);
}

// ---------- routing, FL style ----------
// Select a channel, then click the arrow at the foot of a bus: the channel sends
// some of its sound there (a cable shows it, a small knob above the arrow sets
// how much). Right-click the arrow to route the channel there instead of the
// master. The master's arrow takes it back to the master.
const MAX_SENDS = 4;

// The selected strip, when it can be routed: a song track or bus (an output and
// sends, part of the song), or a live strip, yours or your partner's (sends
// only, to the song's buses, just for your own ears: e.g. one shared reverb
// while you jam).
function routeSource() {
  const t = ui.sel;
  if (!t) return null;
  if (t[0] === "T") {
    const tr = trackBySlot(+t.slice(1));
    return tr && { t, id: tr.id, name: tr.name || "Track", track: tr, sends: tr.sends || {},
      out: tr.out && trackById(tr.out)?.bus ? tr.out : null };
  }
  if (t[0] === "L" || (t[0] === "R" && +t.slice(1) < (ui.remoteCount || 0)))
    return { t, id: t, name: stripName(t), live: true, sends: liveSends()[t] || {}, out: null };
  return null;
}
const liveSends = () => song.view?.local?.livesends || {};

// change a source's sends: the song for a track, this machine for a live strip
function editSends(src, sends, drag = false) {
  if (src.live) {
    song.view.local.livesends = { ...liveSends(), [src.t]: sends };
    clearTimeout(editSends.t);
    const go = () => api.daw_local("livesends", sends, src.t).then((v) => !drag && setSongView(v));
    if (drag) editSends.t = setTimeout(go, 80); else go();
    return;
  }
  src.track.sends = sends;
  if (drag) throttledEdit("track", src.id, { sends }); else songEdit([["track", src.id, { sends }]]);
}

// where a track's sound goes after its fader: the master or a bus (it can't
// be a bus that feeds the track itself)
function outButton(track) {
  const b = document.createElement("button");
  const out = track.out ? trackById(track.out) : null;
  b.className = "sout" + (out?.bus ? " routed" : "");
  b.textContent = "→ " + (out?.bus ? out.name || "Bus" : "Master");
  b.title = "Where it goes after its fader: the master, or a bus (a group: all drums through one compressor)";
  b.onclick = (e) => {
    e.stopPropagation();
    const r = b.getBoundingClientRect();
    const ok = busesOf().filter((x) => x.id !== track.id && !reaches(x.id, track.id));
    showCtx([["Master", "", () => setOut(track, null)], ...ok.map((x) => ["Bus: " + (x.name || "Bus"), "", () => setOut(track, x.id)])], r.left, r.bottom + 2);
  };
  return b;
}

function setOut(track, busId) {
  const ch = { out: busId };
  // a bus it now goes to isn't a send as well
  if (busId && track.sends && busId in track.sends) { const s = { ...track.sends }; delete s[busId]; ch.sends = s; }
  songEdit([["track", track.id, ch]]);
}

// a source's sends with one changed (level 0: gone); null when there's no room
function withSend(src, busId, level) {
  const s = { ...src.sends };
  if (level > 0) {
    if (!(busId in s) && Object.keys(s).length >= MAX_SENDS) { toast(`A channel has up to ${MAX_SENDS} sends.`); return null; }
    s[busId] = level;
  } else delete s[busId];
  src.sends = s;
  return s;
}

// the foot of a strip: the arrow that routes the selected channel here
function routeFoot(t) {
  const foot = document.createElement("div");
  foot.className = "rfoot";
  const src = routeSource();
  const me = t === "M" ? null : t[0] === "T" ? trackBySlot(+t.slice(1)) : null;
  if (!src || !(t === "M" || me?.bus) || me?.id === src.id) return foot;
  if (src.live && !me) return foot; // a live strip always goes to the master
  const loops = me && reaches(me.id, src.id);
  const isOut = me ? src.out === me.id : !src.out;
  const lvl = me ? src.sends[me.id] || 0 : 0;
  const send = (level, drag) => { const s = withSend(src, me.id, level); if (s) editSends(src, s, drag); };
  if (lvl > 0) {
    foot.append(knob({ label: "Send", value: lvl, def: 1, fmt: (v) => fmtDb(v), color: BUS_COLOR, size: 26,
      onChange: (v) => { send(Math.max(0.001, v), true); drawCables(); } }));
  }
  const a = document.createElement("button");
  a.className = "rarrow" + (isOut ? " out" : lvl > 0 ? " send" : "");
  a.disabled = !!loops;
  a.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M12 20 3 8h18z"/></svg>`;
  a.title = loops ? `${me.name} already feeds ${src.name}: that would loop`
    : t === "M" ? (isOut ? `${src.name} goes to the master` : `Route ${src.name} back to the master`)
    : isOut ? `${src.name} goes to ${me.name} (click: back to the master)`
    : lvl > 0 ? `${src.name} sends to ${me.name} (click: stop sending)`
    : `Send ${src.name} to ${me.name}` + (src.live ? " (just for your ears)" : " (right-click: route it there only)");
  a.onclick = (e) => {
    e.stopPropagation();
    if (!me) { if (src.out) setOut(src.track, null); return; }
    if (isOut) setOut(src.track, null);
    else send(lvl > 0 ? 0 : 1);
  };
  a.oncontextmenu = (e) => {
    e.preventDefault(); e.stopPropagation();
    if (!me || loops) return;
    const items = [["Send here", "", () => send(lvl || 1), isOut || lvl > 0]];
    if (!src.live) items.push(["Route here only (not to the master)", "", () => setOut(src.track, me.id), isOut]);
    items.push(["Remove", "", () => (isOut ? setOut(src.track, null) : send(0)), !isOut && !lvl]);
    showCtx(items, e.clientX, e.clientY);
  };
  foot.append(a);
  return foot;
}

// the selection changed: new arrows and cables, without rebuilding the strips
function refreshRouting() {
  for (const el of $("strips").querySelectorAll(".strip")) el.querySelector(".rfoot")?.replaceWith(routeFoot(el.dataset.t));
  drawCables();
}

// cables from the selected channel to where it goes (and, for a bus, from what feeds it)
function drawCables() {
  const box = $("strips");
  box.querySelector("svg.cables")?.remove();
  const sel = routeSource();
  if (!sel) return;
  const at = (t, part) => {
    const el = box.querySelector(`.strip[data-t="${t}"] ${part}`);
    if (!el) return null;
    const r = el.getBoundingClientRect(), b = box.getBoundingClientRect();
    return [r.left + r.width / 2 - b.left + box.scrollLeft, r.bottom - b.top + box.scrollTop];
  };
  const lines = [];
  // from a strip ("T3", "L0"...) to a bus, or (dest null) the master
  const add = (from, dest, kind, lvl = 1) => {
    const to = dest ? "T" + dest.slot : "M";
    const p = at(from, ".rfoot"), q = at(to, ".rarrow") || at(to, ".rfoot");
    if (p && q) lines.push({ p, q, kind, lvl, mine: from === sel.t });
  };
  const sendsFrom = (from, sends) => {
    for (const [b, v] of Object.entries(sends || {})) if (v > 0 && trackById(b)?.bus) add(from, trackById(b), "send", v);
  };
  const wires = (t) => {
    add("T" + t.slot, t.out && trackById(t.out)?.bus ? trackById(t.out) : null, "out");
    sendsFrom("T" + t.slot, t.sends);
  };
  if (sel.live) sendsFrom(sel.t, sel.sends);
  else wires(sel.track);
  // a bus: also what feeds it, tracks and live strips
  if (sel.track?.bus) {
    for (const f of feedersOf(sel.track)) wires(f);
    for (const [strip, sends] of Object.entries(liveSends())) if (sends[sel.id] > 0) add(strip, sel.track, "send", sends[sel.id]);
  }
  if (!lines.length) return;
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.classList.add("cables");
  svg.setAttribute("width", box.scrollWidth);
  svg.setAttribute("height", box.scrollHeight);
  const NS = "http://www.w3.org/2000/svg";
  for (const { p, q, kind, lvl, mine } of lines) {
    // under the strips, from the channel's foot to the arrow it goes into
    const y = Math.max(p[1], q[1]) + 16 + Math.min(8, Math.abs(q[0] - p[0]) / 60);
    const path = document.createElementNS(NS, "path");
    path.setAttribute("d", `M${p[0]} ${p[1] - 2}C${p[0]} ${y},${q[0]} ${y},${q[0]} ${q[1] - 2}`);
    path.setAttribute("class", "cable " + kind + (mine ? "" : " other"));
    path.style.opacity = kind === "send" ? 0.45 + 0.55 * Math.min(1, lvl) : 1;
    const dot = document.createElementNS(NS, "circle");
    dot.setAttribute("cx", p[0]); dot.setAttribute("cy", p[1] - 2); dot.setAttribute("r", 3.5);
    dot.setAttribute("class", "cable-end " + kind);
    svg.append(path, dot);
  }
  box.append(svg);
}

// right-click on an effect's slot
function slotMenu(t, pos, x, y) {
  const n = stripCfg(t).rack.length;
  showCtx([
    ["Open", "", () => openFx(t, pos)],
    [stripCfg(t).rack[pos].on ? "Switch off" : "Switch on", "", () => fxToggle(t, pos)],
    ["Move up", "", () => fxMove(t, pos, pos - 1), pos === 0],
    ["Move down", "", () => fxMove(t, pos, pos + 1), pos >= n - 1],
    ["Remove", "", () => fxRemove(t, pos)],
  ], x, y);
}

async function fxMove(t, a, b) {
  await rackChanged(await api.rack_move(t, a, b));
  fxRekey(t, (p) => (p === a ? b : p === b ? a : p));
}

async function fxRemove(t, pos) {
  fxw.get(fxKey(t, pos))?.el.remove();
  fxw.delete(fxKey(t, pos));
  fxRekey(t, (p) => (p > pos ? p - 1 : p));
  await rackChanged(await api.rack_remove(t, pos));
}

// ---------- an effect's own window ----------
// Like a plugin window in FL: click an effect on a strip and just that effect
// opens, floating over everything, where you can drag it. Several can be open.
const fxw = new Map();
const fxKey = (t, pos) => t + "|" + pos;
let fxZ = 1;

function openFx(t, pos, anchor) {
  const k = fxKey(t, pos);
  const had = fxw.get(k);
  if (had) { had.el.style.zIndex = ++fxZ; had.el.animate([{ transform: "scale(1.03)" }, { transform: "none" }], 160); return; }
  const el = document.createElement("div");
  el.className = "fxwin";
  el.innerHTML = `<div class="fxbar"><button class="fxled" title="On / off"></button><span class="fxt"></span><span class="spacer"></span>
    <button class="wb del" title="Remove this effect"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/></svg></button>
    <button class="wb x" title="Close (Esc)"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"/></svg></button></div><div class="fxbody rack"></div>`;
  const w = { el, t, pos };
  fxw.set(k, w);
  $("fxLayer").append(el);
  el.style.zIndex = ++fxZ;
  el.addEventListener("pointerdown", () => (el.style.zIndex = ++fxZ));
  el.querySelector(".x").onclick = () => fxClose(w);
  el.querySelector(".del").onclick = () => (w.pos === "inst" ? api.inst_set(w.t, null).then((r) => { rackChanged(r); syncMidiTarget(); sendNames(); }) : fxRemove(w.t, w.pos));
  el.querySelector(".fxled").onclick = () => w.pos !== "inst" && fxToggle(w.t, w.pos);
  fxDrag(el);
  fxFill(w);
  // next to the slot it came from, inside the window
  const r = anchor?.getBoundingClientRect?.();
  const n = fxw.size;
  el.style.left = (r ? r.right + 10 : 160 + n * 24) + "px";
  el.style.top = (r ? r.top - 30 : 140 + n * 24) + "px";
  requestAnimationFrame(() => {
    const b = el.getBoundingClientRect();
    if (r && b.right > innerWidth - 8) el.style.left = Math.max(8, r.left - b.width - 10) + "px";
    el.style.left = Math.max(8, Math.min(innerWidth - b.width - 8, parseFloat(el.style.left))) + "px";
    el.style.top = Math.max(8, Math.min(innerHeight - b.height - 8, parseFloat(el.style.top))) + "px";
  });
}

// (re)build a window's contents; it closes when its effect is gone
function fxFill(w) {
  const cfg = stripCfg(w.t);
  const body = w.el.querySelector(".fxbody");
  if (w.pos === "inst") {
    if (!cfg?.inst) { fxClose(w); return; }
    body.replaceChildren(instPanel(w.t, cfg.inst));
    w.el.querySelector(".fxt").textContent = `${instLabel(cfg.inst)} · ${stripName(w.t)}`;
    w.el.querySelector(".fxled").hidden = true;
    w.el.querySelector(".del").title = "Remove the instrument (back to an input)";
    return;
  }
  const slot = cfg?.rack[w.pos];
  if (!slot) { fxClose(w); return; }
  body.replaceChildren(pedalEl(w.t, w.pos, slot));
  const meta = FXMETA[slot.kind];
  const name = slot.kind === "vst" ? slot.vst?.name || "Plugin" : slot.kind === "nam" && slot.model_name ? `${meta.name}: ${slot.model_name}` : meta.name;
  w.el.querySelector(".fxt").textContent = `${name} · ${stripName(w.t)}`;
  w.el.style.setProperty("--fxc", meta.color);
  w.el.querySelector(".fxled").classList.toggle("on", !!slot.on);
}

function fxRefresh() { for (const w of [...fxw.values()]) fxFill(w); }
function fxBody(t, pos) { return fxw.get(fxKey(t, pos))?.el.querySelector(".fxbody"); }
function fxClose(w) { w.el.remove(); fxw.delete(fxKey(w.t, w.pos)); }
function fxCloseStrip(t) { for (const w of [...fxw.values()]) if (w.t === t) fxClose(w); }

// effects moved within a strip: their windows go along
function fxRekey(t, map) {
  const ws = [...fxw.values()].filter((w) => w.t === t && w.pos !== "inst");
  for (const w of ws) fxw.delete(fxKey(t, w.pos));
  for (const w of ws) { w.pos = map(w.pos); fxw.set(fxKey(t, w.pos), w); }
}

function fxDrag(el) {
  const bar = el.querySelector(".fxbar");
  bar.onpointerdown = (e) => {
    if (e.target.closest("button")) return;
    e.preventDefault();
    bar.setPointerCapture(e.pointerId);
    const x0 = e.clientX - el.offsetLeft, y0 = e.clientY - el.offsetTop;
    bar.onpointermove = (ev) => {
      el.style.left = Math.max(-el.offsetWidth + 60, Math.min(innerWidth - 60, ev.clientX - x0)) + "px";
      el.style.top = Math.max(0, Math.min(innerHeight - 30, ev.clientY - y0)) + "px";
    };
    bar.onpointerup = () => (bar.onpointermove = bar.onpointerup = null);
  };
}

// Esc closes the effect window in front
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || !fxw.size || document.querySelector(".modal:not([hidden])")) return;
  const top = [...fxw.values()].sort((a, b) => +b.el.style.zIndex - +a.el.style.zIndex)[0];
  fxClose(top);
});

// a compressor on a song track or the master can listen to another track
function keySelect(t, pos, slot) {
  const ks = document.createElement("select");
  ks.className = "keysel";
  ks.title = "Sidechain: the compressor reacts to another track instead of its own sound (a kick that pushes the bass down)";
  ks.append(new Option("Sidechain: off", ""));
  for (const tr of allTracks()) {
    if (tr.slot == null || "T" + tr.slot === t) continue;
    ks.append(new Option("Sidechain: " + (tr.name || "Track"), tr.slot, false, slot.key === tr.slot));
  }
  ks.onchange = async () => {
    await api.rack_set(t, pos, "key", ks.value === "" ? -1 : +ks.value);
    rackChanged();
  };
  return ks;
}

// ---------- the EQ's curve (the same filters as the engine's, fx.rs) ----------
function eqCoefs(p, sr) {
  const gain = (v) => -15 + 30 * v;
  const ny = sr * 0.45, TAU = 2 * Math.PI;
  const norm = (b, a) => [b[0] / a[0], b[1] / a[0], b[2] / a[0], a[1] / a[0], a[2] / a[0]];
  const hp = (f, q) => { const w = TAU * Math.min(f, ny) / sr, c = Math.cos(w), al = Math.sin(w) / (2 * q); return norm([(1 + c) / 2, -(1 + c), (1 + c) / 2], [1 + al, -2 * c, 1 - al]); };
  const lp = (f, q) => { const w = TAU * Math.min(f, ny) / sr, c = Math.cos(w), al = Math.sin(w) / (2 * q); return norm([(1 - c) / 2, 1 - c, (1 - c) / 2], [1 + al, -2 * c, 1 - al]); };
  const peak = (f, q, db) => {
    const A = 10 ** (db / 40), w = TAU * Math.min(f, ny) / sr, c = Math.cos(w), al = Math.sin(w) / (2 * q);
    return norm([1 + al * A, -2 * c, 1 - al * A], [1 + al / A, -2 * c, 1 - al / A]);
  };
  const shelf = (high, f, db) => {
    const A = 10 ** (db / 40), w = TAU * Math.min(f, ny) / sr, c = Math.cos(w), al = Math.sin(w) / 2 * Math.SQRT2, sq = 2 * Math.sqrt(A) * al;
    return high
      ? norm([A * ((A + 1) + (A - 1) * c + sq), -2 * A * ((A - 1) + (A + 1) * c), A * ((A + 1) + (A - 1) * c - sq)],
             [(A + 1) - (A - 1) * c + sq, 2 * ((A - 1) - (A + 1) * c), (A + 1) - (A - 1) * c - sq])
      : norm([A * ((A + 1) - (A - 1) * c + sq), 2 * A * ((A - 1) - (A + 1) * c), A * ((A + 1) - (A - 1) * c - sq)],
             [(A + 1) + (A - 1) * c + sq, -2 * ((A - 1) + (A + 1) * c), (A + 1) + (A - 1) * c - sq]);
  };
  const out = [];
  if (p.lowcut > 0.005) { const f = 20 * 40 ** p.lowcut; out.push(hp(f, 0.541), hp(f, 1.307)); }
  if (Math.abs(gain(p.low)) >= 0.05) out.push(shelf(false, 30 * 20 ** p.lowfreq, gain(p.low)));
  if (Math.abs(gain(p.mid)) >= 0.05) out.push(peak(100 * 100 ** p.midfreq, 0.3 * 33.3 ** p.q, gain(p.mid)));
  if (Math.abs(gain(p.high)) >= 0.05) out.push(shelf(true, 1500 * 10 ** p.highfreq, gain(p.high)));
  if (p.highcut < 0.995) out.push(lp(1000 * 20 ** p.highcut, Math.SQRT1_2));
  return out;
}

function eqDb(coefs, f, sr) {
  const w = 2 * Math.PI * f / sr;
  const c1 = Math.cos(w), s1 = Math.sin(w), c2 = Math.cos(2 * w), s2 = Math.sin(2 * w);
  let db = 0;
  for (const [b0, b1, b2, a1, a2] of coefs) {
    const num = (b0 + b1 * c1 + b2 * c2) ** 2 + (b1 * s1 + b2 * s2) ** 2;
    const den = (1 + a1 * c1 + a2 * c2) ** 2 + (a1 * s1 + a2 * s2) ** 2;
    db += 10 * Math.log10(num / den);
  }
  return db;
}

// a canvas that draws the slot's curve; .redraw() after a knob turns
function eqCurve(slot, color) {
  const cv = document.createElement("canvas");
  cv.className = "eqcv";
  cv.redraw = () => {
    const w = cv.clientWidth || 286, h = cv.clientHeight || 84;
    const ctx = sizeCanvas(cv, w, h);
    const sr = ui.rate || 48000, RANGE = 18;
    const x = (f) => Math.log(f / 20) / Math.log(1000) * w;
    const y = (db) => h / 2 - db / RANGE * (h / 2 - 5);
    ctx.fillStyle = "rgba(0,0,0,0.32)";
    ctx.fillRect(0, 0, w, h);
    ctx.fillStyle = "rgba(255,255,255,0.07)";
    for (const f of [100, 1000, 10000]) ctx.fillRect(Math.round(x(f)), 0, 1, h);
    for (const db of [-12, 0, 12]) ctx.fillRect(0, Math.round(y(db)), w, 1);
    ctx.fillStyle = "rgba(255,255,255,0.35)";
    ctx.font = "600 9px system-ui, sans-serif";
    ctx.fillText("100", x(100) + 3, h - 4); ctx.fillText("1k", x(1000) + 3, h - 4); ctx.fillText("10k", x(10000) + 3, h - 4);
    const coefs = eqCoefs(slot, sr);
    ctx.beginPath();
    for (let px = 0; px <= w; px++) {
      const f = 20 * 1000 ** (px / w);
      const db = Math.max(-RANGE - 4, Math.min(RANGE + 4, eqDb(coefs, f, sr)));
      px ? ctx.lineTo(px, y(db)) : ctx.moveTo(px, y(db));
    }
    ctx.strokeStyle = color;
    ctx.lineWidth = 2;
    ctx.stroke();
    ctx.lineTo(w, y(0)); ctx.lineTo(0, y(0)); ctx.closePath();
    ctx.fillStyle = color + "30";
    ctx.fill();
  };
  requestAnimationFrame(cv.redraw);
  return cv;
}
