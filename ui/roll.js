"use strict";
// Duet: the piano roll's FL-style parts. What Space plays (PAT: the pattern you're editing, looped;
// SONG: the playlist), the roll's own transport, its ruler and playhead (in both modes: in SONG
// it follows the pattern's block under the song's playhead), its tools and its Tools menu.
//
// Tools (FL's keys): P draw, B paint, D delete, T mute, C slice, E select, Y play.
// Loaded after seq.js.

const PR_TOOLS = [
  ["draw", "P", "Draw (P): click for a note, drag to make it longer; drag a note to move it, its end to stretch it",
    '<path d="M4 20l4-1 11-11-3-3L5 16z"/><path d="M14 6l3 3"/>'],
  ["paint", "B", "Paint (B): hold and drag to paint a note every note length",
    '<path d="M18 3l3 3-9 9-3-3z"/><path d="M9 12c-3 0-5 2-5 5 0 1-1 3-2 3 3 1 7 0 8-3"/>'],
  ["delete", "D", "Delete (D): click or drag over notes to take them away",
    '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>'],
  ["mute", "T", "Mute (T): click or drag over notes to silence them (they stay; again: back on)",
    '<path d="M11 5 6 9H3v6h3l5 4z"/><path d="M22 9l-6 6M16 9l6 6"/>'],
  ["slice", "C", "Slice (C): click a note to cut it in two there (on the grid)",
    '<circle cx="6" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M20 4 8.1 15.9M14.5 14.5 20 20M8.1 8.1 12 12"/>'],
  ["select", "E", "Select (E): drag a box to select; click a note to select it",
    '<rect x="4" y="4" width="16" height="16" rx="2" stroke-dasharray="3 3"/>'],
  ["play", "Y", "Play (Y): drag across to hear the notes under the mouse",
    '<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>'],
];

// --- PAT / SONG -----------------------------------------------------------------------------

function setPlayMode(m, restart) {
  const was = ui.playMode;
  ui.playMode = m === "pat" ? "pat" : "song";
  store.set("playMode", ui.playMode);
  for (const b of $("playMode").querySelectorAll("button")) b.classList.toggle("on", b.dataset.v === ui.playMode);
  const pm = $("prMode");
  if (pm) { pm.textContent = ui.playMode === "pat" ? "PAT" : "SONG"; pm.className = "prmode " + ui.playMode; }
  // switching while it plays: play the other one, as FL does
  if (restart !== false && was && was !== ui.playMode && song.st && song.st.playing && !song.st.recording) startPlaying();
}

async function startPlaying() {
  if (ui.playMode === "pat") {
    if (!seq.pat) seq.pat = patterns()[0]?.id || null;
    if (!seq.pat) { toast("Make a pattern first (the channel rack), or switch to SONG."); return; }
    await api.daw_play_pattern(seq.pat, seq.prCursor || 0);
  } else await api.daw_play(null);
}

// --- the toolbar ----------------------------------------------------------------------------

function rollSetup() {
  setPlayMode(store.get("playMode", "song"), false);
  for (const b of $("playMode").querySelectorAll("button")) b.onclick = () => setPlayMode(b.dataset.v);
  $("prMode").onclick = () => setPlayMode(ui.playMode === "pat" ? "song" : "pat");
  $("prPlay").onclick = () => togglePlay();
  const tools = $("prTools");
  for (const [id, key, title, svg] of PR_TOOLS) {
    const b = document.createElement("button");
    b.dataset.t = id;
    b.title = title;
    b.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${svg}</svg>`;
    b.onclick = () => setPrTool(id);
    tools.append(b);
  }
  setPrTool(store.get("prTool", "draw"));
  $("prMenu").onclick = (e) => { const r = e.currentTarget.getBoundingClientRect(); rollMenu(r.left, r.bottom + 4); };
  seq.follow = store.get("prFollow", true);
  $("prFollow").classList.toggle("on", seq.follow);
  $("prFollow").onclick = () => { seq.follow = !seq.follow; store.set("prFollow", seq.follow); $("prFollow").classList.toggle("on", seq.follow); };
  // the scale to light up: the song's key, any key, or none
  const sc = $("prScale");
  sc.append(new Option("Scale: off", "off"), new Option("Scale: the song's key", "song"));
  for (const k of KEYS) sc.append(new Option("Scale: " + keyLabel(k), k));
  sc.value = store.get("prScaleKey", "song");
  sc.onchange = () => { store.set("prScaleKey", sc.value); prDraw(); };
  // the ruler: play from here
  const ru = $("prRuler");
  ru.addEventListener("pointerdown", rulerDown);
  new ResizeObserver(() => rulerDraw()).observe(ru);
  rollPlayIcon();
}

function rollPlayIcon() {
  const on = !!(song.st && song.st.playing);
  const b = $("prPlay");
  if (!b) return;
  b.classList.toggle("on", on);
  b.innerHTML = on ? `<svg width="13" height="13" viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor"/></svg>`
    : `<svg width="13" height="13" viewBox="0 0 24 24"><path d="M8 5v14l11-7z" fill="currentColor"/></svg>`;
}

// the notes of the scale chosen, as pitch classes (null: none lit)
function prScaleSet() {
  const v = $("prScale")?.value || "off";
  const key = v === "song" ? (typeof songKey === "function" ? songKey() : null) : v === "off" ? null : v;
  return key ? tuneScale(key) : null;
}

// --- where it's playing in this pattern -----------------------------------------------------

// beats into the open pattern at the playhead (null: not playing it)
function prPlayBeat() {
  const st = song.st, p = curPattern();
  if (!st || !st.playing || !p) return null;
  if (song.view.patternMode) return song.view.patternMode === p.id ? curPos() % patLen(p) : null;
  const pos = curPos(), ch = seq.channel;
  const blocks = song.view.clips.filter((c) => c.pattern === p.id && !c.mute);
  // a block on this channel's row first, else any block of the pattern
  const at = (c) => { const len = c.blen || patLen(p); return pos >= c.start && pos < c.start + len; };
  const c = blocks.find((k) => k.track === ch && at(k)) || blocks.find(at);
  if (!c) return null;
  let local = pos - c.start + (c.boff || 0);
  if (!c.once) local %= patRound(p);
  return local;
}

// --- the ruler -------------------------------------------------------------------------------

function rulerDraw() {
  const cv = $("prRuler"), lanes = $("prLanes");
  if (!cv || !winShown("roll")) return;
  const w = cv.clientWidth, h = cv.clientHeight;
  if (!w) return;
  const ctx = sizeCanvas(cv, w, h, false), p = curPattern(), sx = lanes.scrollLeft, ppb = seq.ppb, n = bpb();
  ctx.fillStyle = "#121117"; ctx.fillRect(0, 0, w, h);
  const plen = patLen(p);
  ctx.font = "700 9.5px system-ui, sans-serif";
  for (let b = Math.floor(sx / ppb); b * ppb - sx < w; b++) {
    const x = Math.round(b * ppb - sx) + 0.5, bar = b % n === 0;
    if (!bar && ppb < 22) continue;
    ctx.fillStyle = bar ? "#57526a" : "#2d2a38";
    ctx.fillRect(x - 0.5, bar ? 4 : 12, 1, h);
    if (bar) { ctx.fillStyle = b < plen ? "#bdb6cc" : "#5f586b"; ctx.fillText(String(b / n + 1), x + 3, 11); }
  }
  // past the end: dimmed; the end itself marked
  const xe = plen * ppb - sx;
  if (xe < w) { ctx.fillStyle = "rgba(0,0,0,0.5)"; ctx.fillRect(Math.max(0, xe), 0, w, h); ctx.fillStyle = "#ff9548"; ctx.fillRect(Math.round(xe), 0, 2, h); }
  // where Space starts the pattern from
  if (seq.prCursor) {
    const x = seq.prCursor * ppb - sx;
    ctx.fillStyle = "#4fd6c6"; ctx.beginPath(); ctx.moveTo(x - 5, h); ctx.lineTo(x + 5, h); ctx.lineTo(x, h - 8); ctx.closePath(); ctx.fill();
  }
  const pb = prPlayBeat();
  if (pb != null) {
    const x = pb * ppb - sx;
    ctx.fillStyle = "#fff"; ctx.beginPath(); ctx.moveTo(x - 6, 0); ctx.lineTo(x + 6, 0); ctx.lineTo(x, 9); ctx.closePath(); ctx.fill();
  }
}

async function rulerDown(e) {
  const p = curPattern();
  if (!p || e.button !== 0) return;
  const r = $("prRuler").getBoundingClientRect(), lanes = $("prLanes");
  const beat = Math.max(0, Math.min(patLen(p) - 0.001, Math.floor(((e.clientX - r.left + lanes.scrollLeft) / seq.ppb) / (seq.snap || 1)) * (seq.snap || 1)));
  if (ui.playMode === "pat") {
    seq.prCursor = beat;
    rulerDraw();
    if (song.st && song.st.playing) await api.daw_play_pattern(p.id, beat);
    return;
  }
  // SONG: jump to that beat of the pattern's block in the playlist (the one playing, or the first)
  const blocks = song.view.clips.filter((c) => c.pattern === p.id).sort((a, b) => a.start - b.start);
  if (!blocks.length) { toast("This pattern isn't in the playlist yet: switch to PAT to play it on its own, or put it in the song."); return; }
  const pos = curPos();
  const c = blocks.find((k) => k.track === seq.channel && pos >= k.start && pos < k.start + (k.blen || patLen(p))) || blocks.find((k) => k.track === seq.channel) || blocks[0];
  const at = c.start - (c.boff || 0) + beat;
  if (song.st && song.st.playing) await api.daw_play(Math.max(c.start, at)); else await api.daw_locate(Math.max(c.start, at));
}

// keep the playhead in sight while it plays
function rollFollow() {
  if (!seq.follow || seq.drag) return;
  const pb = prPlayBeat(), lanes = $("prLanes");
  if (pb == null || !lanes) return;
  const x = pb * seq.ppb, w = lanes.clientWidth;
  if (x < lanes.scrollLeft || x > lanes.scrollLeft + w - 40) lanes.scrollLeft = Math.max(0, x - 40);
}

// --- the Tools menu ----------------------------------------------------------------------------

function rollMenu(x, y) {
  const t = prChannel(), p = curPattern();
  if (!t || !p) { toast("Pick a channel and a pattern first."); return; }
  const n = targetNotes().length, what = seq.sel.size ? `${seq.sel.size} selected` : `all ${n}`;
  showCtx([
    ["Import a MIDI file into this pattern…", "", () => midiImport(p.id, t.id)],
    ["Export this pattern as a MIDI file…", "", () => midiExport(p.id)],
    [`Quantize… (${what})`, "Ctrl+Q", quantOpen],
    ["Legato: each note until the next", "Ctrl+L", legato],
    ["Humanize: looser timing and velocity", "Ctrl+H", humanize],
    ["Duplicate the selection after itself", "Ctrl+B", duplicateNotes],
    ["Chop into grid-length pieces", "", rollChop],
    ["Glue touching notes of the same pitch", "", rollGlue],
    ["Strum chords (lowest first)", "", () => rollStrum(1)],
    ["Strum chords (highest first)", "", () => rollStrum(-1)],
    ["Arpeggiate chords (up, on the grid)", "", () => rollArp("up")],
    ["Arpeggiate chords (down)", "", () => rollArp("down")],
    ["Arpeggiate chords (up and down)", "", () => rollArp("updown")],
    ["Flip upside down", "", rollFlipV],
    ["Flip back to front", "", rollFlipH],
    ["An octave up", "", () => rollShift(12)],
    ["An octave down", "", () => rollShift(-12)],
    ["Random velocities (around where they are)", "", rollRandVel],
    ["Velocities 10% louder", "", () => rollScaleVel(1.1)],
    ["Velocities 10% softer", "", () => rollScaleVel(1 / 1.1)],
    ["Velocities: a crescendo", "", () => rollRampVel(true)],
    ["Velocities: a diminuendo", "", () => rollRampVel(false)],
    ["Mute / unmute the selection", "", rollToggleMute],
    ["Select all", "Ctrl+A", () => { for (const k of patNotes(p.id, t.id)) seq.sel.add(k.id); prDraw(); }],
    ["Invert the selection", "", rollInvert],
    ["Select every note at the same pitch", "", rollSameKey],
  ], x, y);
}

const sortNotes = (ns) => ns.slice().sort((a, b) => a.start - b.start || a.key - b.key);
const grid = () => seq.snap || STEP;

function rollChop() {
  const g = grid(), ch = [];
  for (const n of targetNotes()) {
    const pieces = Math.floor(n.len / g + 1e-6);
    if (pieces < 2) continue;
    ch.push(["note", n.id, { len: g }]);
    for (let i = 1; i < pieces; i++) ch.push(["note", newId("n"), { pattern: n.pattern, track: n.track, start: n.start + i * g, len: g, key: n.key, vel: n.vel }]);
  }
  if (ch.length) songEdit(ch); else toast("Nothing longer than the grid to chop.");
}

function rollGlue() {
  const ch = [], byKey = {};
  for (const n of sortNotes(targetNotes())) (byKey[n.key] ||= []).push(n);
  for (const ns of Object.values(byKey)) {
    let cur = null;
    for (const n of ns) {
      if (cur && Math.abs(cur.start + cur.len - n.start) < 1e-4) { cur.len += n.len; ch.push(["note", n.id, { del: true }]); cur.dirty = true; }
      else { if (cur?.dirty) ch.push(["note", cur.id, { len: cur.len }]); cur = { ...n }; }
    }
    if (cur?.dirty) ch.push(["note", cur.id, { len: cur.len }]);
  }
  if (ch.length) songEdit(ch); else toast("No touching notes of the same pitch.");
}

// chords: notes that start together
function chordsOf(ns) {
  const out = [];
  for (const n of sortNotes(ns)) {
    const c = out.find((x) => Math.abs(x.start - n.start) < 1e-4);
    c ? c.notes.push(n) : out.push({ start: n.start, notes: [n] });
  }
  return out.filter((c) => c.notes.length > 1);
}

function rollStrum(dir) {
  const ch = [], step = 1 / 32;
  for (const c of chordsOf(targetNotes())) {
    const ns = c.notes.slice().sort((a, b) => dir * (a.key - b.key));
    ns.forEach((n, i) => i && ch.push(["note", n.id, { start: n.start + i * step, len: Math.max(step, n.len - i * step) }]));
  }
  if (ch.length) songEdit(ch); else toast("No chords to strum (notes that start together).");
}

function rollArp(kind) {
  const g = grid(), ch = [];
  for (const c of chordsOf(targetNotes())) {
    let order = c.notes.slice().sort((a, b) => a.key - b.key);
    if (kind === "down") order.reverse();
    if (kind === "updown" && order.length > 2) order = [...order, ...order.slice(1, -1).reverse()];
    const len = Math.max(...c.notes.map((n) => n.len)), steps = Math.max(1, Math.round(len / g));
    for (const n of c.notes) ch.push(["note", n.id, { del: true }]);
    const tmpl = c.notes[0];
    for (let i = 0; i < steps; i++) {
      const src = order[i % order.length];
      ch.push(["note", newId("n"), { pattern: tmpl.pattern, track: tmpl.track, start: c.start + i * g, len: g, key: src.key, vel: src.vel }]);
    }
  }
  if (ch.length) songEdit(ch); else toast("No chords to arpeggiate (notes that start together).");
}

function rollFlipV() {
  const ns = targetNotes();
  if (!ns.length) return;
  const lo = Math.min(...ns.map((n) => n.key)), hi = Math.max(...ns.map((n) => n.key));
  songEdit(ns.map((n) => ["note", n.id, { key: lo + hi - n.key }]));
}

function rollFlipH() {
  const ns = targetNotes();
  if (!ns.length) return;
  const a = Math.min(...ns.map((n) => n.start)), b = Math.max(...ns.map((n) => n.start + n.len));
  songEdit(ns.map((n) => ["note", n.id, { start: Math.max(0, a + b - n.start - n.len) }]));
}

function rollShift(d) {
  const ns = targetNotes();
  if (ns.some((n) => n.key + d < 0 || n.key + d > 127)) { toast("That would go off the keyboard."); return; }
  songEdit(ns.map((n) => ["note", n.id, { key: n.key + d }]));
}

const clampVel = (v) => Math.max(1, Math.min(127, Math.round(v)));
function rollRandVel() { songEdit(targetNotes().map((n) => ["note", n.id, { vel: clampVel((n.vel ?? 100) + (Math.random() * 2 - 1) * 20) }])); }
function rollScaleVel(f) { songEdit(targetNotes().map((n) => ["note", n.id, { vel: clampVel((n.vel ?? 100) * f) }])); }
function rollRampVel(up) {
  const ns = sortNotes(targetNotes());
  if (ns.length < 2) return;
  const a = ns[0].start, b = ns[ns.length - 1].start || 1;
  songEdit(ns.map((n) => { const u = (n.start - a) / Math.max(1e-6, b - a); return ["note", n.id, { vel: clampVel(30 + 97 * (up ? u : 1 - u)) }]; }));
}
function rollToggleMute() {
  const ns = targetNotes();
  if (!ns.length) return;
  const to = !ns.every((n) => n.mute);
  songEdit(ns.map((n) => ["note", n.id, { mute: to }]));
}
function rollInvert() {
  const all = patNotes(seq.pat, seq.channel);
  seq.sel = new Set(all.filter((n) => !seq.sel.has(n.id)).map((n) => n.id));
  prDraw();
}
function rollSameKey() {
  const all = patNotes(seq.pat, seq.channel);
  const keys = new Set(all.filter((n) => seq.sel.has(n.id)).map((n) => n.key));
  if (!keys.size && seq.mouse) keys.add(seq.mouse.key);
  seq.sel = new Set(all.filter((n) => keys.has(n.key)).map((n) => n.id));
  prDraw();
}

// --- MIDI files -------------------------------------------------------------------------------

async function midiImport(pattern, track) {
  const r = await api.midi_import(pattern || null, track || null, pattern ? 0 : curPos());
  if (!r) return;
  if (r.error) { toast(r.error, true); return; }
  if (pattern) toast(`${r.notes} note${r.notes === 1 ? "" : "s"} brought in.`);
  else {
    seq.pat = r.pattern;
    toast(`Brought in ${r.notes} notes on ${r.channels.length} new channel${r.channels.length === 1 ? "" : "s"} (${r.channels.join(", ")}), as a pattern in the playlist.`);
  }
}

async function midiExport(pattern) {
  const r = await api.midi_export(pattern || null);
  if (!r) return;
  if (r.error) toast(r.error, true);
  else toast(`Saved ${r.notes} notes on ${r.tracks} track${r.tracks === 1 ? "" : "s"}: ${r.path.split(/[\\/]/).pop()}`);
}

// --- the tools on the roll (called from prDown for anything but draw and paint) -----------------

// true if the tool took the click
function rollToolDown(e, pt, p, t) {
  const tool = seq.tool, lanes = $("prLanes");
  if (tool === "delete") { prErase(e); return true; }
  if (tool === "slice") {
    const h = prHit(pt);
    if (!h) return true;
    const at = Math.round(pt.beat / grid()) * grid();
    if (at <= h.n.start + 1e-6 || at >= h.n.start + h.n.len - 1e-6) { toast("Slice inside the note, on the grid (a finer grid slices smaller)."); return true; }
    songEdit([["note", h.n.id, { len: at - h.n.start }],
      ["note", newId("n"), { pattern: h.n.pattern, track: h.n.track, start: at, len: h.n.start + h.n.len - at, key: h.n.key, vel: h.n.vel, mute: !!h.n.mute }]]);
    return true;
  }
  if (tool === "mute") {
    lanes.setPointerCapture(e.pointerId);
    const first = prHit(pt), to = first ? !first.n.mute : true, done = new Set();
    const hit = (ev) => {
      const h = prHit(prPoint(ev));
      if (h && !done.has(h.n.id)) { done.add(h.n.id); h.n.mute = to; prDraw(); }
    };
    const up = () => {
      lanes.removeEventListener("pointermove", hit); lanes.removeEventListener("pointerup", up);
      if (done.size) songEdit([...done].map((id) => ["note", id, { mute: to }]));
    };
    lanes.addEventListener("pointermove", hit); lanes.addEventListener("pointerup", up);
    hit(e);
    return true;
  }
  if (tool === "play") {
    lanes.setPointerCapture(e.pointerId);
    let last = "";
    const play = (ev) => {
      const q = prPoint(ev);
      const under = patNotes(p.id, t.id).filter((n) => !n.mute && q.beat >= n.start && q.beat < n.start + n.len);
      const sig = under.map((n) => n.id).join();
      if (sig === last) return;
      last = sig;
      releaseHeld();
      for (const n of under) audition(t, n.key, n.vel);
      seq.scrub = q.beat; prDraw();
    };
    const up = () => { lanes.removeEventListener("pointermove", play); lanes.removeEventListener("pointerup", up); releaseHeld(); seq.scrub = null; prDraw(); };
    lanes.addEventListener("pointermove", play); lanes.addEventListener("pointerup", up);
    play(e);
    return true;
  }
  return false;
}
