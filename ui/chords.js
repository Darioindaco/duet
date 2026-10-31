"use strict";
// Duet: chords in the piano roll.
//
// * Browse chords on a note: select one note, hold Shift and press ↑/↓ (or scroll). Every chord on that
//   note plays and shows where it would go; ←/→ (still holding Shift) turn it to its inversions. Let go of
//   Shift and that chord stays. Esc puts the note back as it was. Select the chord again and the same
//   keys go on from where you left it.
// * Chord progressions (the Chords button): a key, a progression (by degree, so it fits any key), how long
//   each chord lasts, triads or sevenths, and they're written into the pattern, voiced close together.
//
// The song's key (meta.key, shared) is what both start from; chords that fit it come first.
//
// Loaded after seq.js; seqSetup() calls chordSetup().

// [name, intervals from the root]
const CHORD_TYPES = [
  ["", [0, 4, 7]], ["m", [0, 3, 7]], ["sus2", [0, 2, 7]], ["sus4", [0, 5, 7]], ["dim", [0, 3, 6]], ["aug", [0, 4, 8]],
  ["5", [0, 7]], ["6", [0, 4, 7, 9]], ["m6", [0, 3, 7, 9]], ["7", [0, 4, 7, 10]], ["maj7", [0, 4, 7, 11]], ["m7", [0, 3, 7, 10]],
  ["m7♭5", [0, 3, 6, 10]], ["dim7", [0, 3, 6, 9]], ["mMaj7", [0, 3, 7, 11]], ["7sus4", [0, 5, 7, 10]], ["add9", [0, 4, 7, 14]],
  ["madd9", [0, 3, 7, 14]], ["6/9", [0, 4, 7, 9, 14]], ["9", [0, 4, 7, 10, 14]], ["maj9", [0, 4, 7, 11, 14]], ["m9", [0, 3, 7, 10, 14]],
  ["7♭9", [0, 4, 7, 10, 13]], ["7♯9", [0, 4, 7, 10, 15]], ["11", [0, 4, 7, 10, 14, 17]], ["m11", [0, 3, 7, 10, 14, 17]],
  ["maj7♯11", [0, 4, 7, 11, 18]], ["13", [0, 4, 7, 10, 14, 21]], ["maj13", [0, 4, 7, 11, 14, 21]], ["m13", [0, 3, 7, 10, 14, 21]],
];
const CH_SCALES = { major: [0, 2, 4, 5, 7, 9, 11], minor: [0, 2, 3, 5, 7, 8, 10] };
const CH_NAMES = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];

const songKey = () => { const k = song.view?.meta?.key; return typeof k === "string" && KEYS.includes(k) ? k : null; };
function keyPcs(key) {
  if (!key) return null;
  const [root, mode] = key.split(" ");
  const r = NOTE_NAMES_JS.indexOf(root);
  return new Set(CH_SCALES[mode].map((i) => (r + i) % 12));
}
const chordName = (root, type) => CH_NAMES[((root % 12) + 12) % 12] + type[0];

// The chords on a root, the ones that fit the key first (a single note is the first and the last: browsing
// round comes back to where it started).
function chordList(root) {
  const pcs = keyPcs(songKey());
  const fits = (t) => !pcs || t[1].every((i) => pcs.has((root + i) % 12));
  const types = CHORD_TYPES.map((t) => ({ t, fit: fits(t) }));
  const ordered = pcs ? [...types.filter((x) => x.fit), ...types.filter((x) => !x.fit)] : types;
  return [{ t: ["", [0]], single: true, fit: true }, ...ordered];
}

// The keys of a chord on `root` in an inversion (0 = root position): the lowest notes go up an octave.
function voice(root, intervals, inv) {
  const ks = intervals.map((i) => root + i).sort((a, b) => a - b);
  const n = ((inv % ks.length) + ks.length) % ks.length;
  for (let i = 0; i < n; i++) ks.push(ks.shift() + 12);
  return ks.sort((a, b) => a - b);
}

// ---------- browsing ----------
const chb = { s: null, last: null, held: [] };

function chordStart() {
  const t = prChannel();
  if (!t || isDrums(t) || !seq.pat) return null;
  const sel = patNotes(seq.pat, t.id).filter((n) => seq.sel.has(n.id));
  if (!sel.length) return null;
  // the chord made last time, selected again: go on from there
  const L = chb.last;
  if (L && sel.length === L.ids.length && sel.every((n) => L.ids.includes(n.id))) {
    const base = sel.find((n) => n.id === L.rootId) || sel[0];
    return { t, base, others: sel.filter((n) => n.id !== base.id), root: L.root, list: chordList(L.root), idx: L.idx, inv: L.inv };
  }
  if (sel.length !== 1) return null;
  return { t, base: sel[0], others: [], root: sel[0].key, list: chordList(sel[0].key), idx: 0, inv: 0 };
}

function chordKeys(s) {
  const c = s.list[s.idx];
  return voice(s.root, c.t[1], c.single ? 0 : s.inv).filter((k) => k >= 0 && k <= 127);
}

function chordHear(s) {
  for (const k of chb.held) api.note("T" + s.t.slot, false, k);
  chb.held = [];
  if (s.t.slot == null) return;
  for (const k of chordKeys(s)) { api.note("T" + s.t.slot, true, k, s.base.vel ?? 100); chb.held.push(k); }
}
function chordSilence() {
  const s = chb.s;
  if (s && s.t.slot != null) for (const k of chb.held) api.note("T" + s.t.slot, false, k);
  chb.held = [];
}

// ↑/↓ or scroll with Shift held: the next / the previous chord; ←/→: its inversions
function chordStep(dir, invDir = 0) {
  if (!chb.s) {
    chb.s = chordStart();
    if (!chb.s) return false;
  }
  const s = chb.s;
  if (invDir) { if (!s.list[s.idx].single) s.inv += invDir; }
  else { s.idx = (s.idx + dir + s.list.length) % s.list.length; s.inv = 0; }
  chordHear(s);
  prDraw();
  return true;
}

// Shift let go: that chord stays (the note itself keeps its id; the others are new, or reused)
function chordCommit() {
  const s = chb.s;
  if (!s) return;
  chordSilence();
  chb.s = null;
  const keys = chordKeys(s), c = s.list[s.idx];
  const ch = [];
  const ids = [s.base.id];
  ch.push(["note", s.base.id, { key: keys[0] }]);
  const spare = s.others.slice();
  for (const k of keys.slice(1)) {
    const n = spare.shift();
    if (n) { ch.push(["note", n.id, { key: k, start: s.base.start, len: s.base.len }]); ids.push(n.id); }
    else {
      const id = newId("n");
      ids.push(id);
      ch.push(["note", id, { pattern: seq.pat, track: s.t.id, start: s.base.start, len: s.base.len, key: k, vel: s.base.vel ?? 100 }]);
    }
  }
  for (const n of spare) ch.push(["note", n.id, { del: true }]);
  songEdit(ch);
  seq.sel = new Set(ids);
  chb.last = { ids, rootId: s.base.id, root: s.root, idx: s.idx, inv: s.inv };
  if (!c.single) toast(`${chordName(s.root, c.t)}${s.inv ? ` (inversion ${((s.inv % keys.length) + keys.length) % keys.length})` : ""}`);
  prDraw();
}

function chordCancel() {
  if (!chb.s) return;
  chordSilence();
  chb.s = null;
  prDraw();
}

// what's being browsed, drawn over the roll: the chord's notes outlined, and its name
function chordDrawPreview(ctx, sx, sy, hi, ppb, rh, col) {
  const s = chb.s;
  if (!s) return;
  const keys = chordKeys(s), c = s.list[s.idx];
  const x = s.base.start * ppb - sx, nw = Math.max(3, s.base.len * ppb - 1);
  // the notes being replaced, dimmed
  ctx.fillStyle = "rgba(16,15,20,0.75)";
  for (const n of [s.base, ...s.others]) ctx.fillRect(n.start * ppb - sx, (hi - n.key) * rh - sy + 1, Math.max(3, n.len * ppb), rh - 2);
  for (const k of keys) {
    const y = (hi - k) * rh - sy;
    ctx.globalAlpha = 0.85;
    ctx.fillStyle = col;
    ctx.beginPath(); ctx.roundRect(x + 0.5, y + 1.5, nw, rh - 3, 3); ctx.fill();
    ctx.globalAlpha = 1;
    ctx.setLineDash([3, 2]);
    ctx.strokeStyle = "#fff";
    ctx.lineWidth = 1.2;
    ctx.stroke();
    ctx.setLineDash([]);
  }
  const top = (hi - Math.max(...keys)) * rh - sy;
  const label = c.single ? "single note" : chordName(s.root, c.t) + (((s.inv % keys.length) + keys.length) % keys.length ? ` · inv ${((s.inv % keys.length) + keys.length) % keys.length}` : "");
  const sub = c.single ? "" : songKey() ? (c.fit ? `in ${keyLabel(songKey())}` : "outside the key") : "";
  ctx.font = "700 12px system-ui, sans-serif";
  const tw = Math.max(ctx.measureText(label).width, sub ? ctx.measureText(sub).width * 0.9 : 0) + 14;
  const ly = Math.max(4, top - (sub ? 38 : 26));
  ctx.fillStyle = "rgba(10,9,13,0.92)";
  ctx.beginPath(); ctx.roundRect(x, ly, tw, sub ? 34 : 22, 6); ctx.fill();
  ctx.strokeStyle = col; ctx.lineWidth = 1; ctx.stroke();
  ctx.fillStyle = "#f4f0f8";
  ctx.fillText(label, x + 7, ly + 15);
  if (sub) { ctx.font = "600 10px system-ui, sans-serif"; ctx.fillStyle = c.fit ? "#7fd8b0" : "#e7a25a"; ctx.fillText(sub, x + 7, ly + 28); }
  ctx.font = "600 10px system-ui, sans-serif";
  ctx.fillStyle = "rgba(244,240,248,0.55)";
  ctx.fillText(`${s.idx + 1}/${s.list.length} · ↑↓ chord · ←→ inversion · let go of Shift`, x, Math.min(ly + (sub ? 48 : 36), (hi - Math.min(...keys) + 1) * rh - sy + 14));
}

// ---------- progressions ----------
// by degree: upper case major, lower case minor, ° diminished, ♭ a semitone down; the numerals count in the
// key's own scale (in a minor key, III is the minor scale's third)
const PROGRESSIONS = {
  major: [
    ["Pop", "I V vi IV"], ["50s / doo-wop", "I vi IV V"], ["Sad pop", "vi IV I V"], ["Jazz turnaround", "ii V I I"],
    ["Rock", "I IV V IV"], ["Anthem", "I IV vi V"], ["Royal road", "IV V iii vi"], ["Canon", "I V vi iii IV I IV V"],
    ["Uplifting", "I iii IV V"], ["Mixolydian", "I ♭VII IV I"], ["Blues (12 bars)", "I I I I IV IV I I V IV I V"], ["Soul", "ii iii IV V"],
  ],
  minor: [
    ["Epic", "i VI III VII"], ["Classic", "i iv v i"], ["Andalusian", "i VII VI V"], ["Dark pop", "i iv VII III"],
    ["Cinematic", "i VI VII i"], ["Trap", "i iv VI V"], ["Minor ii–V–i", "ii° V i i"], ["Ballad", "i III VII VI"],
    ["Drill", "i VI iv V"], ["Lament", "i VII VI VII"],
  ],
};
const ROMAN = ["I", "II", "III", "IV", "V", "VI", "VII"];

// a degree in a key: { root (pitch class), quality "maj" | "min" | "dim" }
function degree(tok, key) {
  const [rootName, mode] = key.split(" ");
  const r = NOTE_NAMES_JS.indexOf(rootName);
  const flat = tok.startsWith("♭") || tok.startsWith("b");
  const t = tok.replace(/^[♭b]/, "");
  const dim = t.endsWith("°");
  const num = t.replace("°", "");
  const i = ROMAN.indexOf(num.toUpperCase());
  const off = CH_SCALES[mode][i] - (flat ? 1 : 0);
  return { root: (r + off + 12) % 12, quality: dim ? "dim" : num === num.toUpperCase() ? "maj" : "min", label: tok };
}

function degreeChord(d, sevenths, isV) {
  const tri = { maj: [0, 4, 7], min: [0, 3, 7], dim: [0, 3, 6] }[d.quality];
  if (!sevenths) return tri;
  return d.quality === "dim" ? [0, 3, 6, 10] : d.quality === "min" ? [0, 3, 7, 10] : isV ? [0, 4, 7, 10] : [0, 4, 7, 11];
}
const degreeName = (d, sevenths, isV) => CH_NAMES[d.root] + ({ maj: sevenths ? (isV ? "7" : "maj7") : "", min: sevenths ? "m7" : "m", dim: sevenths ? "m7♭5" : "dim" })[d.quality];

// the progression's chords as keys, each voiced close to the one before (the smallest move)
function progressionKeys(key, prog, octave, sevenths, bass) {
  const out = [];
  let prev = null;
  for (const tok of prog.split(" ")) {
    const d = degree(tok, key), isV = tok.replace(/[♭b°]/g, "") === "V";
    const iv = degreeChord(d, sevenths, isV);
    const base = 12 * (octave + 1) + d.root;
    let best = null;
    for (const rootOct of [base - 12, base]) {
      for (let inv = 0; inv < iv.length; inv++) {
        const ks = voice(rootOct, iv, inv);
        const centre = ks.reduce((a, k) => a + k, 0) / ks.length;
        const target = prev ? prev.reduce((a, k) => a + k, 0) / prev.length : 12 * (octave + 1) + 7;
        const cost = Math.abs(centre - target) + (prev ? 0 : inv * 0.5);
        if (!best || cost < best.cost) best = { ks, cost };
      }
    }
    prev = best.ks;
    const keys = bass ? [12 * octave + d.root, ...best.ks] : best.ks;
    out.push({ keys, name: degreeName(d, sevenths, isV), label: tok });
  }
  return out;
}

function chordsPanel() {
  const old = $("chordPop");
  if (old) { old.remove(); return; }
  const t = prChannel();
  if (!t || isDrums(t)) { toast("Pick a melodic channel (not drums) to write chords for.", true); return; }
  const pop = document.createElement("div");
  pop.id = "chordPop";
  pop.className = "chordpop";
  const key = songKey() || "C major";
  const opts = (list, sel) => list.map(([v, l]) => `<option value="${esc(String(v))}"${String(v) === String(sel) ? " selected" : ""}>${esc(l)}</option>`).join("");
  const st = store.get("chordPanel", { len: 4, octave: 4, sevenths: false, bass: false });
  pop.innerHTML = `<div class="cphead"><b>Chord progression</b><button class="qx" aria-label="Close">✕</button></div>
    <div class="cprow"><label>Key</label><select class="cpkey">${opts(KEYS.map((k) => [k, keyLabel(k)]), key)}</select>
      <span class="fine cpkeyhint">${songKey() ? "the song's key" : "sets the song's key"}</span></div>
    <div class="cplist"></div>
    <div class="cprow"><label>Each chord</label><select class="cplen">${opts([[1, "1 beat"], [2, "2 beats"], [4, "1 bar"], [8, "2 bars"]], st.len)}</select>
      <label>Octave</label><select class="cpoct">${opts([[2, "2"], [3, "3"], [4, "4"], [5, "5"]], st.octave)}</select></div>
    <div class="cprow"><label class="tcheck"><input type="checkbox" class="cp7"${st.sevenths ? " checked" : ""}> Sevenths</label>
      <label class="tcheck"><input type="checkbox" class="cpbass"${st.bass ? " checked" : ""}> Bass note</label></div>
    <div class="cprow"><span class="fine cpwhere"></span><button class="btn primary cpgo">Write it</button></div>`;
  $("win-roll").querySelector(".rollcard").append(pop);
  const $p = (s) => pop.querySelector(s);
  let chosen = store.get("chordProg", null);
  const settings = () => ({ key: $p(".cpkey").value, len: +$p(".cplen").value, octave: +$p(".cpoct").value, sevenths: $p(".cp7").checked, bass: $p(".cpbass").checked });
  const at = () => {
    const sel = patNotes(seq.pat || "", t.id).filter((n) => seq.sel.has(n.id));
    return sel.length ? Math.min(...sel.map((n) => n.start)) : 0;
  };
  const fill = () => {
    const s = settings(), mode = s.key.split(" ")[1];
    const list = PROGRESSIONS[mode];
    if (!chosen || !list.some(([, p]) => p === chosen)) chosen = list[0][1];
    $p(".cplist").innerHTML = "";
    for (const [name, prog] of list) {
      const b = document.createElement("button");
      b.className = "cpitem" + (prog === chosen ? " on" : "");
      const chords = progressionKeys(s.key, prog, s.octave, s.sevenths, false);
      b.innerHTML = `<b>${esc(name)}</b><span class="cpdeg">${esc(prog.replace(/ /g, " – "))}</span><span class="cpnames">${esc(chords.map((c) => c.name).join(" – "))}</span>`;
      b.onclick = () => { chosen = prog; store.set("chordProg", prog); fill(); hear(); };
      $p(".cplist").append(b);
    }
    $p(".cpwhere").textContent = `From beat ${+(at() + 1).toFixed(2)} (${seq.sel.size ? "where the selection starts" : "the pattern's start"})`;
  };
  // hear the progression, a chord at a time
  let playing = [];
  const hear = () => {
    for (const id of playing) clearTimeout(id);
    playing = [];
    if (t.slot == null) return;
    const s = settings(), chords = progressionKeys(s.key, chosen, s.octave, s.sevenths, s.bass);
    chords.slice(0, 8).forEach((c, i) => {
      playing.push(setTimeout(() => { for (const k of c.keys) api.note("T" + t.slot, true, k, 90); }, i * 420));
      playing.push(setTimeout(() => { for (const k of c.keys) api.note("T" + t.slot, false, k); }, i * 420 + 380));
    });
  };
  for (const sel of [".cpkey", ".cplen", ".cpoct", ".cp7", ".cpbass"]) {
    $p(sel).onchange = () => {
      const s = settings();
      store.set("chordPanel", { len: s.len, octave: s.octave, sevenths: s.sevenths, bass: s.bass });
      if (sel === ".cpkey") songEdit([["meta", "meta", { key: s.key }]]);
      fill();
    };
  }
  $p(".qx").onclick = () => pop.remove();
  $p(".cpgo").onclick = () => {
    const s = settings();
    if (!songKey() || songKey() !== s.key) songEdit([["meta", "meta", { key: s.key }]]);
    if (!curPattern()) newPattern();
    const chords = progressionKeys(s.key, chosen, s.octave, s.sevenths, s.bass);
    const start = at(), notes = [];
    chords.forEach((c, i) => { for (const k of c.keys) notes.push({ start: start + i * s.len, len: s.len, key: k, vel: 96 }); });
    const end = start + chords.length * s.len;
    const p = curPattern();
    if (p && end > patLen(p)) songEdit([["pattern", p.id, { len: Math.ceil(end / bpb()) * bpb() }]]);
    placeNotes(notes, t);
    toast(`${chords.map((c) => c.name).join(" – ")} written.`);
    pop.remove();
  };
  fill();
}

// ---------- setup ----------
function chordSetup() {
  $("prChords").onclick = chordsPanel;
  // Shift let go: the chord stays; the window losing focus counts as letting go
  addEventListener("keyup", (e) => { if (e.key === "Shift") chordCommit(); });
  addEventListener("blur", chordCommit);
}

// called by seqKeys: true if it was a chord key
function chordKey(e) {
  if (e.key === "Escape" && chb.s) { chordCancel(); return true; }
  if (!e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return false;
  if (e.key === "ArrowUp" || e.key === "ArrowDown") return chordStep(e.key === "ArrowUp" ? 1 : -1);
  if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && chb.s) return chordStep(0, e.key === "ArrowRight" ? 1 : -1);
  return false;
}
// called by prWheel: true if it was a chord scroll
function chordWheel(e) {
  if (!e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return false;
  const now = performance.now();
  if (chb.wheelAt && now - chb.wheelAt < 90) return !!chb.s; // one chord per notch, not per pixel of a trackpad
  const dir = (e.deltaY || e.deltaX) < 0 ? 1 : -1;
  if (!chordStep(dir)) return false;
  chb.wheelAt = now;
  return true;
}
