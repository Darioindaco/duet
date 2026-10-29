// The clip editor's Tune tab: a voice's notes, Melodyne-style, with an
// assistant that knows the song's key.
//
// The voice is listened to (its pitch every few ms, cut into notes); each
// note is a blob on a piano roll at the pitch it's heard at, its shape its
// loudness, the pitch line through it. Drag a note to move it (it snaps to
// semitones, or to the key; Alt: free), double-click it to put it in key.
// "Snap to key" does that for every note that's off (or the selected ones),
// as much as Strength says, and can take out a note's drift while leaving its
// vibrato: tuned, not auto-tuned. Notes already within the tolerance, short
// ones (slides, ornaments) and chromatic notes that were clearly meant are
// left alone.
//
// What's moved is kept in the clip (clip.tune: the moved notes), so it's in
// the shared song: the partner's engine makes the same tuned voice.

const NOTE_NAMES_JS = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const TUNE_SCALES = { major: [0, 2, 4, 5, 7, 9, 11], minor: [0, 2, 3, 5, 7, 8, 10] };
const FLAT_NAMES = { "C#": "D♭", "D#": "E♭", "G#": "A♭", "A#": "B♭" };
const tn = {
  clip: null, pitch: null, loading: false, error: null, detected: null, detecting: false,
  sel: new Set(), hover: -1, drag: null, band: null,
  strength: 0.85, drift: 0.3, tolerance: 12, keepChromatic: true, scaleSnap: true,
};

// "D# major" -> "E♭ major" (how keys are usually written)
function keyLabel(k) {
  if (!k) return "Chromatic (no key)";
  const [root, mode] = k.split(" ");
  const flat = mode === "major" ? FLAT_NAMES[root] : root === "A#" ? "B♭" : root === "D#" ? "E♭" : null;
  return `${(flat || root).replace("#", "♯")} ${mode}`;
}
const KEYS = (() => {
  const out = [];
  for (const r of NOTE_NAMES_JS) for (const m of ["major", "minor"]) out.push(`${r} ${m}`);
  return out;
})();
function midiName(m) {
  const r = Math.round(m);
  return NOTE_NAMES_JS[((r % 12) + 12) % 12].replace("#", "♯") + (Math.floor(r / 12) - 1);
}

// --- the notes: the analysis, with the clip's moves on top ------------------------------

const tuneOf = (c) => (c && c.tune && typeof c.tune === "object" ? c.tune : { key: undefined, notes: [] });
const tuneKey = (c) => { const k = tuneOf(c).key; return k === undefined ? tn.detected && tn.detected.key : k; };

function tuneScale(key) {
  if (!key) return null;
  const [root, mode] = key.split(" ");
  const r = NOTE_NAMES_JS.indexOf(root);
  return new Set(TUNE_SCALES[mode].map((d) => (d + r) % 12));
}

// [{start, end, center, shift, drift, i}], from the analysis and the clip's moves
function tuneNotes(c) {
  if (!tn.pitch) return [];
  const moves = tuneOf(c).notes || [];
  return tn.pitch.notes.map(([start, end, center], i) => {
    const m = moves.find((x) => Math.abs(x[0] - start) < 0.003 && Math.abs(x[1] - end) < 0.003);
    return { start, end, center, shift: m ? m[3] : 0, drift: m ? m[4] : 0, i };
  });
}

function tuneSave(c, notes, key) {
  const t = tuneOf(c);
  const moved = notes.filter((n) => Math.abs(n.shift) > 1e-4 || n.drift > 1e-4)
    .map((n) => [+n.start.toFixed(5), +n.end.toFixed(5), +n.center.toFixed(4), +n.shift.toFixed(4), +n.drift.toFixed(3)]);
  const tune = { key: key === undefined ? t.key : key, notes: moved };
  ceEdit({ tune: tune.notes.length || tune.key !== undefined ? tune : null });
}

// where a note belongs: the nearest note of the key (or semitone), unless it
// was sung cleanly on a note outside the key (that was meant)
function tuneTarget(center, scale) {
  const near = Math.round(center);
  if (!scale) return near;
  if (tn.keepChromatic && Math.abs(center - near) < 0.2) return near;
  let best = near, bd = 99;
  for (let m = Math.floor(center) - 2; m <= Math.ceil(center) + 2; m++) {
    if (scale.has(((m % 12) + 12) % 12) && Math.abs(m - center) < bd) { best = m; bd = Math.abs(m - center); }
  }
  return best;
}

// --- loading -----------------------------------------------------------------------

async function tuneLoad(c) {
  const sig = `${c.id}|${c.file}|${(c.offset || 0).toFixed(4)}|${ceSrcLen(c).toFixed(4)}`;
  if (tn.sig === sig && (tn.pitch || tn.loading)) return;
  tn.sig = sig; tn.clip = c.id; tn.pitch = null; tn.error = null; tn.loading = true; tn.sel.clear();
  tuneRender();
  const r = await api.daw_analyze(c.id, "pitch");
  if (tn.sig !== sig) return;
  tn.loading = false;
  if (!r || r.error) { tn.error = r ? r.error : "Couldn't listen to the clip."; tuneRender(); return; }
  tn.pitch = r.pitch;
  // the view: the clip's stretch of the file, the voice's range
  tuneRender();
  if (tuneOf(c).key === undefined && !tn.detected) tuneDetect(c);
}

async function tuneDetect(c) {
  tn.detecting = true; tuneRender();
  const r = await api.daw_analyze(c.id, "key");
  tn.detecting = false;
  if (r && !r.error) tn.detected = r;
  tuneRender();
}

// --- the panel ---------------------------------------------------------------------

function tuneTab(c) {
  const box = document.createElement("div");
  box.className = "tunepanel";
  tuneLoad(c);
  const notes = tuneNotes(c), scale = tuneScale(tuneKey(c));
  // the key: detected (how sure), or chosen
  const sel = document.createElement("select");
  sel.className = "cenum tkey";
  sel.title = "The song's key: notes are put into it";
  const t = tuneOf(c);
  sel.append(new Option(tn.detected && tn.detected.key ? `Detected: ${keyLabel(tn.detected.key)}` : tn.detecting ? "Detecting…" : "Detected: —", "auto", false, t.key === undefined));
  for (const k of KEYS) sel.append(new Option(keyLabel(k), k, false, t.key === k));
  sel.append(new Option("Chromatic (no key)", "none", false, t.key === null));
  sel.onchange = () => tuneSave(c, notes, sel.value === "auto" ? undefined : sel.value === "none" ? null : sel.value);
  const sure = document.createElement("span");
  sure.className = "fine";
  const d = tn.detected;
  // its relative key (C major / A minor) has the same notes: say so rather than "not sure"
  const rel = (k) => { const [r, m] = k.split(" "), i = NOTE_NAMES_JS.indexOf(r); return m === "major" ? `${NOTE_NAMES_JS[(i + 9) % 12]} minor` : `${NOTE_NAMES_JS[(i + 3) % 12]} major`; };
  const twin = d && d.key && d.others[0] === rel(d.key);
  sure.textContent = !d || !d.key ? "" : twin ? `the same notes as ${keyLabel(d.others[0])}` + (d.confidence < 0.3 ? " (either fits)" : "")
    : d.confidence >= 0.6 ? "pretty sure" : d.confidence >= 0.3 ? "fairly sure" : `not sure: maybe ${d.others.slice(0, 2).map(keyLabel).join(" or ")}`;
  box.append(ceRow("Key", sel, sure, ceBtn("Detect again", "Listen to the song again for its key", () => { tn.detected = null; tuneDetect(c); })));
  // the assistant
  const off = notes.filter((n) => n.end - n.start >= 0.08 && Math.abs(tuneTarget(n.center, scale) - (n.center + n.shift)) * 100 > tn.tolerance);
  const which = tn.sel.size ? `${tn.sel.size} selected note${tn.sel.size === 1 ? "" : "s"}` : "every note that's off";
  box.append(ceRow("Assist", ceBtn("✦ Snap to key", `Into ${scale ? keyLabel(tuneKey(c)) : "the nearest note"}: ${which}`, () => tuneSnap(c), "btn teal"),
    tuneVal(tn.sel.size ? `${tn.sel.size} selected` : notes.length ? `${off.length} of ${notes.length} notes are off` : "")));
  const pct = (v) => Math.round(v * 100) + "%";
  box.append(ceRow("Strength", ceRange(tn.strength, (v) => { tn.strength = v; tuneRefresh(); }, "How far a note goes toward its target: 100% is right on it"), tuneVal(pct(tn.strength), "str")));
  box.append(ceRow("Smooth drift", ceRange(tn.drift, (v) => { tn.drift = v; tuneRefresh(); }, "Takes out a note's slow wandering (vibrato stays)"), tuneVal(pct(tn.drift), "dr")));
  box.append(ceRow("Leave within", ceRange(tn.tolerance / 50, (v) => { tn.tolerance = Math.round(v * 50); tuneRefresh(); }, "Notes this close to their target aren't touched"), tuneVal(`±${tn.tolerance} cents`, "tol")));
  const keep = document.createElement("label");
  keep.className = "tcheck";
  keep.innerHTML = `<input type="checkbox" ${tn.keepChromatic ? "checked" : ""}> Leave notes outside the key that were clearly meant`;
  keep.querySelector("input").onchange = (e) => { tn.keepChromatic = e.target.checked; tuneRender(); };
  const snap = document.createElement("label");
  snap.className = "tcheck";
  snap.innerHTML = `<input type="checkbox" ${tn.scaleSnap ? "checked" : ""}> Dragged notes snap to the key`;
  snap.querySelector("input").onchange = (e) => { tn.scaleSnap = e.target.checked; };
  box.append(ceRow("", keep, snap));
  box.append(ceRow("", ceBtn("Back to how it was sung", "Undo every move on this clip", () => { tn.sel.clear(); tuneSave(c, notes.map((n) => ({ ...n, shift: 0, drift: 0 }))); })));
  const hint = document.createElement("p");
  hint.className = "fine";
  hint.textContent = "Drag a note up or down (Alt: free) · double-click: into the key · ↑ ↓ a semitone · Delete: back as sung · drag on empty space to select · scroll to zoom";
  box.append(hint);
  return box;
}

function tuneVal(text, id) {
  const s = document.createElement("span");
  s.className = "ceval";
  if (id) s.dataset.tv = id;
  s.textContent = text;
  return s;
}
function tuneRefresh() {
  const set = (id, v) => { const el = document.querySelector(`[data-tv="${id}"]`); if (el) el.textContent = v; };
  set("str", Math.round(tn.strength * 100) + "%");
  set("dr", Math.round(tn.drift * 100) + "%");
  set("tol", `±${tn.tolerance} cents`);
}

function tuneRender() { if (ce.tab === "tune") ceRender(); }

function tuneSnap(c) {
  const scale = tuneScale(tuneKey(c));
  const notes = tuneNotes(c).map((n) => {
    if (tn.sel.size && !tn.sel.has(n.i)) return n;
    if (n.end - n.start < 0.08) return n; // a slide or an ornament
    const target = tuneTarget(n.center, scale), dev = target - n.center;
    if (Math.abs(dev) * 100 <= tn.tolerance && !tn.sel.size) return { ...n, drift: tn.drift > 0 ? tn.drift : n.drift };
    return { ...n, shift: dev * tn.strength, drift: tn.drift };
  });
  tuneSave(c, notes);
  const moved = notes.filter((n) => Math.abs(n.shift) > 1e-4).length;
  toast(moved ? `${moved} note${moved === 1 ? "" : "s"} put into ${scale ? keyLabel(tuneKey(c)) : "tune"}.` : "Every note is in tune already.");
}

// --- the piano roll -------------------------------------------------------------------

const TUNE_GUTTER = 46;

function tuneRange(c) {
  const ns = tuneNotes(c);
  if (!ns.length) return { lo: 55, hi: 79 };
  const ps = ns.flatMap((n) => [n.center, n.center + n.shift]);
  const lo = Math.floor(Math.min(...ps)) - 3, hi = Math.ceil(Math.max(...ps)) + 3;
  return hi - lo < 14 ? { lo: Math.floor((lo + hi) / 2) - 7, hi: Math.floor((lo + hi) / 2) + 7 } : { lo, hi };
}

function tuneGeom(c, w, h) {
  const { a, b } = ce.view, { lo, hi } = tuneRange(c);
  const rowH = h / (hi - lo + 1);
  return {
    x: (t) => TUNE_GUTTER + (t - a) / (b - a) * (w - TUNE_GUTTER),
    t: (px) => a + (px - TUNE_GUTTER) / (w - TUNE_GUTTER) * (b - a),
    y: (m) => (hi - m + 0.5) * rowH, m: (py) => hi + 0.5 - py / rowH, rowH, lo, hi,
  };
}

function tuneDraw() {
  const c = ceClip(), cv = $("tuneCv");
  if (!c || !cv || ce.tab !== "tune") return;
  const w = cv.clientWidth, h = cv.clientHeight;
  if (!w || !h) return;
  const ctx = sizeCanvas(cv, w, h), g = tuneGeom(c, w, h);
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#0e0d12";
  ctx.fillRect(0, 0, w, h);
  const key = tuneKey(c), scale = tuneScale(key), root = key ? NOTE_NAMES_JS.indexOf(key.split(" ")[0]) : -1;
  // rows: the key's notes lighter, its root lighter still
  for (let m = g.lo; m <= g.hi; m++) {
    const pc = ((m % 12) + 12) % 12, y = g.y(m) - g.rowH / 2;
    ctx.fillStyle = pc === root ? "#1f1c28" : scale && scale.has(pc) ? "#17151e" : !scale && [0, 2, 4, 5, 7, 9, 11].includes(pc) ? "#17151e" : "#111016";
    ctx.fillRect(TUNE_GUTTER, y, w - TUNE_GUTTER, g.rowH);
    ctx.fillStyle = "#1c1a22";
    ctx.fillRect(TUNE_GUTTER, y, w - TUNE_GUTTER, 1);
    // the keyboard on the left
    const black = [1, 3, 6, 8, 10].includes(pc);
    ctx.fillStyle = black ? "#2a2732" : "#d8d3e0";
    ctx.fillRect(0, y + 0.5, TUNE_GUTTER - 4, g.rowH - 1);
    if (pc === 0 || g.rowH >= 13) {
      ctx.fillStyle = black ? "#cfc8da" : "#2a2732";
      ctx.font = `${pc === 0 ? 700 : 500} ${Math.min(11, g.rowH - 2)}px system-ui, sans-serif`;
      ctx.fillText(midiName(m), 4, y + g.rowH / 2 + 4);
    }
    if (scale && scale.has(pc)) { ctx.fillStyle = pc === root ? "#4fd6c6" : "#4fd6c655"; ctx.fillRect(TUNE_GUTTER - 4, y + 1, 3, g.rowH - 2); }
  }
  // the clip's stretch of the file
  const o = c.offset || 0, L = ceSrcLen(c);
  ctx.fillStyle = "rgba(0,0,0,0.35)";
  ctx.fillRect(TUNE_GUTTER, 0, Math.max(0, g.x(o) - TUNE_GUTTER), h);
  ctx.fillRect(g.x(o + L), 0, w, h);
  if (!tn.pitch) {
    ctx.fillStyle = "#b9b2c4";
    ctx.font = "600 13px system-ui, sans-serif";
    ctx.fillText(tn.error || (tn.loading ? "Listening to the voice…" : ""), TUNE_GUTTER + 16, h / 2);
    return;
  }
  const p = tn.pitch, notes = tuneNotes(c), col = tColor(song.view.tracks.find((t) => t.id === c.track));
  const frameAt = (t) => Math.round((t - p.start) / p.hop);
  // the blobs: their outline is how loud the voice is
  for (const n of notes) {
    const x0 = g.x(n.start), x1 = g.x(n.end);
    if (x1 < TUNE_GUTTER || x0 > w) continue;
    const sel = tn.sel.has(n.i), moved = Math.abs(n.shift) > 1e-4;
    const target = tuneTarget(n.center, scale), now = n.center + n.shift;
    const off = Math.abs(target - now) * 100 > tn.tolerance && n.end - n.start >= 0.08;
    if (moved) { // where it was sung
      ctx.strokeStyle = "#ffffff30";
      ctx.setLineDash([3, 3]);
      ctx.strokeRect(x0, g.y(n.center) - g.rowH * 0.4, x1 - x0, g.rowH * 0.8);
      ctx.setLineDash([]);
    }
    const yc = g.y(now), half = g.rowH * 0.48;
    const f0 = frameAt(n.start), f1 = frameAt(n.end), steps = Math.max(2, Math.min(80, Math.round((x1 - x0) / 3)));
    const amp = (u) => {
      const f = Math.round(f0 + u * (f1 - f0));
      return 0.3 + 0.7 * Math.min(1, (p.rms[f] || 0) * 1.6);
    };
    ctx.beginPath();
    for (let s = 0; s <= steps; s++) { const u = s / steps; ctx.lineTo(x0 + u * (x1 - x0), yc - half * amp(u)); }
    for (let s = steps; s >= 0; s--) { const u = s / steps; ctx.lineTo(x0 + u * (x1 - x0), yc + half * amp(u)); }
    ctx.closePath();
    ctx.fillStyle = sel ? col + "e8" : off ? "#ffc857a8" : col + "90";
    ctx.fill();
    ctx.strokeStyle = sel ? "#ffffff" : tn.hover === n.i ? "#ffffffb0" : "#00000060";
    ctx.lineWidth = sel ? 1.5 : 1;
    ctx.stroke();
  }
  // the pitch line, as it will sound (moves and drift included)
  ctx.strokeStyle = "#ffffffd0";
  ctx.lineWidth = 1.4;
  ctx.beginPath();
  let pen = false;
  const fa = Math.max(0, frameAt(ce.view.a)), fb = Math.min(p.midi.length - 1, frameAt(ce.view.b));
  const stepF = Math.max(1, Math.floor((fb - fa) / (w - TUNE_GUTTER)));
  let ni = 0;
  for (let f = fa; f <= fb; f += stepF) {
    const m = p.midi[f], t = p.start + f * p.hop;
    while (ni < notes.length && notes[ni].end < t) ni++;
    const n = notes[ni] && notes[ni].start <= t ? notes[ni] : null;
    if (m == null) { pen = false; continue; }
    let v = m;
    if (n) v = m + n.shift + n.drift * (n.center - tuneSlow(f));
    const x = g.x(t), y = g.y(v);
    if (pen) ctx.lineTo(x, y); else ctx.moveTo(x, y);
    pen = true;
  }
  ctx.stroke();
  // selection box
  if (tn.band) {
    const b = tn.band;
    ctx.fillStyle = "rgba(79,214,198,0.12)";
    ctx.strokeStyle = "#4fd6c6";
    ctx.fillRect(Math.min(b.x0, b.x1), Math.min(b.y0, b.y1), Math.abs(b.x1 - b.x0), Math.abs(b.y1 - b.y0));
    ctx.strokeRect(Math.min(b.x0, b.x1), Math.min(b.y0, b.y1), Math.abs(b.x1 - b.x0), Math.abs(b.y1 - b.y0));
  }
  // the playhead
  if (song.st && song.st.playing) {
    const t = (curPos() - c.start) * spb();
    if (t >= 0 && t <= c.len) { ctx.fillStyle = "#fff"; ctx.fillRect(g.x(clipSrcAt(c, t)), 0, 1.5, h); }
  }
}

// the slow line through the pitch (150 ms), for previewing drift taken out
function tuneSlow(f) {
  const p = tn.pitch, w = Math.round(0.075 / p.hop);
  if (!tn.slow || tn.slow.src !== p) {
    const out = new Float32Array(p.midi.length);
    for (let k = 0; k < p.midi.length; k++) {
      let s = 0, n = 0;
      for (let j = Math.max(0, k - w); j <= Math.min(p.midi.length - 1, k + w); j++) if (p.midi[j] != null) { s += p.midi[j]; n++; }
      out[k] = n ? s / n : NaN;
    }
    tn.slow = { src: p, v: out };
  }
  const v = tn.slow.v[f];
  return Number.isFinite(v) ? v : p.midi[f];
}

function tuneHit(c, px, py, w, h) {
  const g = tuneGeom(c, w, h);
  for (const n of tuneNotes(c)) {
    if (px >= g.x(n.start) - 2 && px <= g.x(n.end) + 2 && Math.abs(py - g.y(n.center + n.shift)) <= g.rowH * 0.55) return n;
  }
  return null;
}

function tuneSetup() {
  const cv = $("tuneCv");
  const at = (e) => { const r = cv.getBoundingClientRect(); return { px: e.clientX - r.left, py: e.clientY - r.top, w: r.width, h: r.height }; };
  cv.addEventListener("pointermove", (e) => {
    const c = ceClip();
    if (!c || tn.drag || tn.band || !tn.pitch) return;
    const q = at(e), n = tuneHit(c, q.px, q.py, q.w, q.h);
    cv.style.cursor = n ? "ns-resize" : "default";
    if ((n ? n.i : -1) !== tn.hover) { tn.hover = n ? n.i : -1; tuneDraw(); }
    if (n) {
      const now = n.center + n.shift, cents = Math.round((n.center - Math.round(n.center)) * 100);
      cv.title = `${midiName(n.center)} ${cents >= 0 ? "+" : ""}${cents} ct as sung` + (Math.abs(n.shift) > 1e-4 ? ` → ${midiName(now)} ${Math.round((now - Math.round(now)) * 100)} ct` : "");
    } else cv.title = "";
  });
  cv.addEventListener("dblclick", (e) => {
    const c = ceClip(), q = at(e), n = c && tuneHit(c, q.px, q.py, q.w, q.h);
    if (!n) return;
    const scale = tuneScale(tuneKey(c));
    tuneSave(c, tuneNotes(c).map((x) => (x.i === n.i ? { ...x, shift: tuneTarget(x.center, scale) - x.center } : x)));
  });
  cv.addEventListener("pointerdown", (e) => {
    const c = ceClip();
    if (!c || e.button !== 0 || !tn.pitch) return;
    cv.setPointerCapture(e.pointerId);
    const q = at(e), n = tuneHit(c, q.px, q.py, q.w, q.h), g = tuneGeom(c, q.w, q.h);
    if (!n) {
      // a box on empty space selects
      if (!e.shiftKey) tn.sel.clear();
      tn.band = { x0: q.px, y0: q.py, x1: q.px, y1: q.py };
      const keep = new Set(tn.sel);
      const move = (ev) => {
        const r = at(ev);
        tn.band.x1 = r.px; tn.band.y1 = r.py;
        const [xa, xb] = [Math.min(tn.band.x0, r.px), Math.max(tn.band.x0, r.px)], [ya, yb] = [Math.min(tn.band.y0, r.py), Math.max(tn.band.y0, r.py)];
        tn.sel = new Set(keep);
        for (const x of tuneNotes(c)) if (g.x(x.end) > xa && g.x(x.start) < xb && g.y(x.center + x.shift) > ya && g.y(x.center + x.shift) < yb) tn.sel.add(x.i);
        tuneDraw();
      };
      const up = () => { cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up); tn.band = null; tuneRender(); };
      cv.addEventListener("pointermove", move);
      cv.addEventListener("pointerup", up);
      tuneDraw();
      return;
    }
    if (e.shiftKey) { tn.sel.has(n.i) ? tn.sel.delete(n.i) : tn.sel.add(n.i); tuneRender(); return; }
    if (!tn.sel.has(n.i)) { tn.sel.clear(); tn.sel.add(n.i); }
    const start = tuneNotes(c), scale = tuneScale(tuneKey(c)), m0 = g.m(q.py);
    tn.drag = { moved: false };
    ce.busy = true;
    const move = (ev) => {
      const r = at(ev), delta = g.m(r.py) - m0;
      if (!tn.drag.moved && Math.abs(r.py - q.py) < 3) return;
      tn.drag.moved = true;
      const notes = start.map((x) => {
        if (!tn.sel.has(x.i)) return x;
        let to = x.center + x.shift + delta;
        if (!ev.altKey) {
          if (tn.scaleSnap && scale) {
            let best = Math.round(to), bd = 99;
            for (let m = Math.floor(to) - 2; m <= Math.ceil(to) + 2; m++) if (scale.has(((m % 12) + 12) % 12) && Math.abs(m - to) < bd) { best = m; bd = Math.abs(m - to); }
            to = best;
          } else to = Math.round(to);
        }
        return { ...x, shift: to - x.center };
      });
      tuneSave(c, notes);
      tuneDraw();
    };
    const up = () => { cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up); tn.drag = null; ce.busy = false; tuneRender(); };
    cv.addEventListener("pointermove", move);
    cv.addEventListener("pointerup", up);
    tuneDraw();
  });
  cv.addEventListener("wheel", (e) => {
    const c = ceClip();
    if (!c) return;
    e.preventDefault();
    const r = cv.getBoundingClientRect(), dur = ceFileDur(c), { a, b } = ce.view, span = b - a;
    const frac = Math.max(0, (e.clientX - r.left - TUNE_GUTTER) / (r.width - TUNE_GUTTER));
    if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
      const d = (e.shiftKey ? e.deltaY : e.deltaX) / r.width * span;
      const na = Math.max(0, Math.min(dur - span, a + d));
      ce.view = { a: na, b: na + span };
    } else {
      const at2 = a + frac * span, f = Math.exp(e.deltaY * 0.002), ns = Math.max(0.05, Math.min(dur, span * f));
      const na = Math.max(0, Math.min(dur - ns, at2 - frac * ns));
      ce.view = { a: na, b: na + ns };
    }
    tuneDraw();
  }, { passive: false });
  // the keys, while the Tune tab is open
  addEventListener("keydown", (e) => {
    if ($("clipEd").hidden || ce.tab !== "tune" || e.target.closest("input, select, textarea")) return;
    const c = ceClip();
    if (!c || !tn.pitch) return;
    const notes = tuneNotes(c), scale = tuneScale(tuneKey(c));
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") { e.preventDefault(); e.stopPropagation(); tn.sel = new Set(notes.map((n) => n.i)); tuneRender(); return; }
    if (e.key === "Escape" && tn.sel.size) { e.preventDefault(); e.stopPropagation(); tn.sel.clear(); tuneRender(); return; }
    if (!tn.sel.size) return;
    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      e.preventDefault(); e.stopPropagation();
      const dir = e.key === "ArrowUp" ? 1 : -1;
      tuneSave(c, notes.map((n) => {
        if (!tn.sel.has(n.i)) return n;
        let to = Math.round(n.center + n.shift) + dir;
        if (tn.scaleSnap && scale && !e.altKey) while (!scale.has(((to % 12) + 12) % 12)) to += dir;
        return { ...n, shift: to - n.center };
      }));
    } else if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault(); e.stopPropagation();
      tuneSave(c, notes.map((n) => (tn.sel.has(n.i) ? { ...n, shift: 0, drift: 0 } : n)));
    }
  }, true);
  addEventListener("resize", tuneDraw);
}
