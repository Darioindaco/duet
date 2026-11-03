// Duet Tune: a voice's notes in a window of its own, the way a note editor for audio works.
//
// Every note the voice sings is a blob on a piano roll, as wide as it lasts and shaped by how loud it
// is, with the pitch line running through it. Tools (keys 1-4):
//   Pitch    drag a note up or down (snaps to the scale or semitones; Alt: free); double-click: into key
//   Vibrato  drag up for more of its wobble, down for less (0 = straight); double-click: as sung
//   Drift    drag up to pull its slow wandering onto the centre; double-click: all or nothing
//   Level    drag up or down for louder or softer; double-click: as sung
// Pressing a note plays it, and every step it's dragged to plays again, so you hear where it goes.
// Click the ruler to put the play cursor there; Space plays from it (again: stop), the playhead
// runs through the notes. Wheel: zoom in time; Shift+wheel: scroll; Alt+wheel: zoom the pitch.
//
// The moves live in the clip (clip.tune.notes: [start, end, centre, shift, drift, vibrato, dB]), so
// they're in the shared song and the partner's engine makes the same voice.

"use strict";

const tw = {
  el: null, clipId: null, pitch: null, loading: false, error: null, sig: null,
  view: { a: 0, b: 1 }, lo: null, hi: null, sel: new Set(), hover: -1, drag: null, band: null,
  tool: "pitch", snap: "scale", cursor: null, follow: true, active: false, raf: 0, lastAud: null,
};
const TW_KEYS = 56;   // the keyboard on the left, px
const TW_RULER = 22;  // the time ruler on top, px
const TW_TOOLS = [["pitch", "Pitch", "1", "Move notes up and down"], ["mod", "Vibrato", "2", "More or less of a note's wobble"],
  ["drift", "Drift", "3", "Pull a note's slow wandering onto its centre"], ["gain", "Level", "4", "Make a note louder or softer"]];

const twClip = () => (tw.clipId && song.view ? song.view.clips.find((c) => c.id === tw.clipId) || null : null);

// --- the notes: the analysis with the clip's moves on top ---------------------------------

function twNotes(c) {
  if (!tw.pitch) return [];
  const moves = tuneOf(c).notes || [];
  return tw.pitch.notes.map(([start, end, center], i) => {
    const m = moves.find((x) => Math.abs(x[0] - start) < 0.003 && Math.abs(x[1] - end) < 0.003);
    return { start, end, center, i, shift: m ? +m[3] : 0, drift: m ? +m[4] : 0, mod: m && m.length > 5 ? +m[5] : 1, gain: m && m.length > 6 ? +m[6] : 0 };
  });
}

function twSave(c, notes, key) {
  const t = tuneOf(c);
  const moved = notes.filter((n) => Math.abs(n.shift) > 1e-4 || n.drift > 1e-4 || Math.abs(n.mod - 1) > 1e-3 || Math.abs(n.gain) > 0.01)
    .map((n) => [+n.start.toFixed(5), +n.end.toFixed(5), +n.center.toFixed(4), +n.shift.toFixed(4), +n.drift.toFixed(3), +n.mod.toFixed(3), +n.gain.toFixed(2)]);
  const tune = { key: key === undefined ? t.key : key, notes: moved };
  const fields = { tune: tune.notes.length || tune.key !== undefined ? tune : null };
  Object.assign(c, fields);
  throttledEdit("clip", c.id, fields);
  songDraw();
  twDraw();
  twInspect();
}

const twKey = (c) => { const k = tuneOf(c).key; return k === undefined ? tn.detected && tn.detected.key : k; };

// --- opening -------------------------------------------------------------------------------

function openTune(clipId) {
  const c = song.view && song.view.clips.find((k) => k.id === clipId);
  if (!c || c.pattern) return;
  if (!tw.el) twBuild();
  if (tw.clipId !== clipId) {
    tw.clipId = clipId; tw.sel.clear(); tw.lo = tw.hi = null; tw.cursor = c.offset || 0;
    const o = c.offset || 0, L = ceSrcLen(c), dur = ceFileDur(c), pad = Math.max(0.2, L * 0.05);
    tw.view = { a: Math.max(0, o - pad), b: Math.min(dur, o + L + pad) };
  }
  tw.el.hidden = false;
  tw.el.style.zIndex = 60;
  twLoad(c);
  twChrome();
  requestAnimationFrame(twDraw);
}

function closeTune() {
  if (!tw.el) return;
  tw.el.hidden = true;
  tw.active = false;
  api.tune_audition_stop?.();
}

async function twLoad(c) {
  const sig = `${c.id}|${c.file}|${(c.offset || 0).toFixed(4)}|${ceSrcLen(c).toFixed(4)}`;
  if (tw.sig === sig && (tw.pitch || tw.loading)) return;
  tw.sig = sig; tw.pitch = null; tw.error = null; tw.loading = true;
  twDraw();
  const r = await api.daw_analyze(c.id, "pitch");
  if (tw.sig !== sig) return;
  tw.loading = false;
  if (!r || r.error) { tw.error = r ? r.error : "Couldn't listen to the clip."; twDraw(); return; }
  tw.pitch = r.pitch;
  tw.slow = null;
  twChrome(); twDraw(); twInspect();
  if (tuneOf(c).key === undefined && !tn.detected) {
    tn.detecting = true; twChrome();
    const k = await api.daw_analyze(c.id, "key");
    tn.detecting = false;
    if (k && !k.error) tn.detected = k;
    twChrome(); twDraw();
  }
}

// --- the window ------------------------------------------------------------------------------

function twBuild() {
  const el = document.createElement("div");
  el.className = "twin";
  el.hidden = true;
  el.innerHTML = `
    <div class="twbar"><span class="twdot"></span><span class="twt">Tune</span><span class="spacer"></span>
      <button class="wb x" title="Close (Esc)"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg></button></div>
    <div class="twtools">
      <button class="twplay" title="Play from the cursor (Space)"><svg width="13" height="13" viewBox="0 0 24 24" fill="currentColor"><path d="M7 4v16l13-8z"/></svg></button>
      <button class="twfollow on" title="The view follows the playhead">Follow</button>
      <span class="twsep"></span>
      <div class="twseg tools">${TW_TOOLS.map(([k, n, key, t]) => `<button data-t="${k}" title="${t} (${key})">${n}</button>`).join("")}</div>
      <span class="twsep"></span>
      <label class="twl">Snap</label><div class="twseg snap"><button data-s="off" title="Free">Off</button><button data-s="semi" title="To semitones">Semitone</button><button data-s="scale" title="To the notes of the key">Scale</button></div>
      <label class="twl">Key</label><select class="twkey"></select><span class="twsure"></span>
      <span class="spacer"></span>
      <button class="twbtn twassist" title="Put every note that's off into the key (or the selected ones)">✦ Snap to key</button>
      <button class="twbtn twreset" title="Undo every move on the selected notes (or all)">Reset</button>
    </div>
    <div class="locknote" id="twLock" hidden></div>
    <div class="twmain"><canvas class="twcv"></canvas></div>
    <div class="twinfo"></div>
    <div class="twgrip" title="Drag to resize"></div>`;
  document.body.append(el);
  tw.el = el;
  const geo = store.get("tunewin", null) || { x: 80, y: 70, w: Math.min(1180, innerWidth - 160), h: Math.min(640, innerHeight - 140) };
  Object.assign(el.style, { left: geo.x + "px", top: geo.y + "px", width: geo.w + "px", height: geo.h + "px" });
  const keepGeo = () => store.set("tunewin", { x: el.offsetLeft, y: el.offsetTop, w: el.offsetWidth, h: el.offsetHeight });
  // drag by the bar, size from the corner
  el.querySelector(".twbar").onpointerdown = (e) => {
    if (e.target.closest("button")) return;
    const bar = e.currentTarget, x0 = e.clientX - el.offsetLeft, y0 = e.clientY - el.offsetTop;
    bar.setPointerCapture(e.pointerId);
    bar.onpointermove = (ev) => { el.style.left = Math.max(-el.offsetWidth + 80, Math.min(innerWidth - 80, ev.clientX - x0)) + "px"; el.style.top = Math.max(0, Math.min(innerHeight - 40, ev.clientY - y0)) + "px"; };
    bar.onpointerup = () => { bar.onpointermove = bar.onpointerup = null; keepGeo(); };
  };
  el.querySelector(".twgrip").onpointerdown = (e) => {
    const g = e.currentTarget, w0 = el.offsetWidth, h0 = el.offsetHeight, x0 = e.clientX, y0 = e.clientY;
    g.setPointerCapture(e.pointerId);
    g.onpointermove = (ev) => { el.style.width = Math.max(560, w0 + ev.clientX - x0) + "px"; el.style.height = Math.max(320, h0 + ev.clientY - y0) + "px"; twDraw(); };
    g.onpointerup = () => { g.onpointermove = g.onpointerup = null; keepGeo(); };
  };
  el.querySelector(".x").onclick = closeTune;
  el.querySelector(".twplay").onclick = twPlay;
  el.querySelector(".twfollow").onclick = (e) => { tw.follow = !tw.follow; e.currentTarget.classList.toggle("on", tw.follow); };
  el.querySelectorAll(".tools button").forEach((b) => (b.onclick = () => { tw.tool = b.dataset.t; twChrome(); }));
  el.querySelectorAll(".snap button").forEach((b) => (b.onclick = () => { tw.snap = b.dataset.s; twChrome(); }));
  el.querySelector(".twkey").onchange = (e) => {
    const c = twClip(); if (!c) return;
    const v = e.target.value;
    twSave(c, twNotes(c), v === "auto" ? undefined : v === "none" ? null : v);
  };
  el.querySelector(".twassist").onclick = (e) => twAssist(e.currentTarget);
  el.querySelector(".twreset").onclick = () => {
    const c = twClip(); if (!c) return;
    twSave(c, twNotes(c).map((n) => (!tw.sel.size || tw.sel.has(n.i) ? { ...n, shift: 0, drift: 0, mod: 1, gain: 0 } : n)));
  };
  el.addEventListener("pointerdown", () => { tw.active = true; el.style.zIndex = 60; }, true);
  document.addEventListener("pointerdown", (e) => { if (!el.contains(e.target) && !e.target.closest(".pkpick, .menu")) tw.active = false; }, true);
  new ResizeObserver(() => twDraw()).observe(el.querySelector(".twmain"));
  twCanvas();
  twKeys();
}

// the bar's state: title, tools, snap, key
function twChrome() {
  const el = tw.el, c = twClip();
  if (!el || !c) return;
  const tr = song.view.tracks.find((t) => t.id === c.track), f = song.view.files[c.file];
  el.querySelector(".twt").textContent = `Tune · ${(f && f.name) || "clip"}${tr ? " · " + (tr.name || "Track") : ""}`;
  el.querySelector(".twdot").style.background = tColor(tr);
  el.querySelectorAll(".tools button").forEach((b) => b.classList.toggle("on", b.dataset.t === tw.tool));
  el.querySelectorAll(".snap button").forEach((b) => b.classList.toggle("on", b.dataset.s === tw.snap));
  const sel = el.querySelector(".twkey"), t = tuneOf(c), d = tn.detected;
  sel.replaceChildren(new Option(d && d.key ? `Detected: ${keyLabel(d.key)}` : tn.detecting ? "Detecting…" : "Detected: —", "auto", false, t.key === undefined));
  for (const k of KEYS) sel.append(new Option(keyLabel(k), k, false, t.key === k));
  sel.append(new Option("Chromatic (no key)", "none", false, t.key === null));
  el.querySelector(".twsure").textContent = d && d.key && t.key === undefined ? (d.confidence >= 0.6 ? "sure" : d.confidence >= 0.3 ? "fairly sure" : "not sure") : "";
  el.querySelector(".twmain").dataset.tool = tw.tool;
}

// the strip under the roll: the selected note(s), or how to use it
function twInspect() {
  const el = tw.el, c = twClip();
  if (!el || !c) return;
  const box = el.querySelector(".twinfo"), notes = twNotes(c), scale = tuneScale(twKey(c));
  const sel = notes.filter((n) => tw.sel.has(n.i));
  if (!tw.pitch) { box.textContent = tw.error || (tw.loading ? "Listening to the voice…" : ""); return; }
  if (!sel.length) {
    const off = notes.filter((n) => n.end - n.start >= 0.08 && Math.abs(tuneTarget(n.center, scale) - (n.center + n.shift)) * 100 > tn.tolerance).length;
    box.innerHTML = `<span><b>${notes.length}</b> notes · <b>${off}</b> off the ${scale ? "key" : "semitones"}</span>
      <span class="fine">Click a note to hear it · drag to move · 1-4 tools · Space plays from the cursor · click the ruler to place it · Ctrl+A all · Delete: as sung</span>`;
    return;
  }
  const one = sel.length === 1 ? sel[0] : null;
  const cents = (m) => { const d = Math.round((m - Math.round(m)) * 100); return (d >= 0 ? "+" : "") + d + " ct"; };
  const avg = (f) => sel.reduce((a, n) => a + f(n), 0) / sel.length;
  box.innerHTML = one
    ? `<span class="twnote">${midiName(one.center + one.shift)}</span><span>${cents(one.center + one.shift)}</span>
       <span class="fine">sung ${midiName(one.center)} ${cents(one.center)}</span>
       <span>moved <b>${(one.shift >= 0 ? "+" : "") + one.shift.toFixed(2)}</b> st</span>
       <span>vibrato <b>${Math.round(one.mod * 100)}%</b></span><span>drift out <b>${Math.round(one.drift * 100)}%</b></span>
       <span>level <b>${(one.gain >= 0 ? "+" : "") + one.gain.toFixed(1)} dB</b></span><span class="fine">${(one.end - one.start).toFixed(2)} s</span>`
    : `<span><b>${sel.length}</b> notes selected</span><span>vibrato <b>${Math.round(avg((n) => n.mod) * 100)}%</b></span>
       <span>drift out <b>${Math.round(avg((n) => n.drift) * 100)}%</b></span><span>level <b>${avg((n) => n.gain).toFixed(1)} dB</b></span>`;
}

// the assistant: how strongly, how much drift, what to leave
function twAssist(anchor) {
  const c = twClip();
  if (!c || !tw.pitch) return;
  document.querySelector(".twpop")?.remove();
  const pop = document.createElement("div");
  pop.className = "twpop";
  const which = tw.sel.size ? `the ${tw.sel.size} selected note${tw.sel.size === 1 ? "" : "s"}` : "every note that's off";
  pop.innerHTML = `<div class="twph">Snap to key: ${esc(which)}</div>
    <label>Strength <input type="range" min="0" max="100" value="${Math.round(tn.strength * 100)}" data-k="strength"><b></b></label>
    <label>Take out drift <input type="range" min="0" max="100" value="${Math.round(tn.drift * 100)}" data-k="drift"><b></b></label>
    <label>Leave within <input type="range" min="0" max="50" value="${tn.tolerance}" data-k="tol"><b></b></label>
    <label class="twchk"><input type="checkbox" ${tn.keepChromatic ? "checked" : ""}> leave notes outside the key that were clearly meant</label>
    <div class="twpa"><button class="twbtn go">Snap</button><button class="twbtn no">Close</button></div>`;
  const show = () => pop.querySelectorAll("input[type=range]").forEach((i) => { i.nextElementSibling.textContent = i.dataset.k === "tol" ? `±${i.value} ct` : i.value + "%"; });
  pop.querySelectorAll("input[type=range]").forEach((i) => (i.oninput = () => {
    if (i.dataset.k === "strength") tn.strength = i.value / 100;
    else if (i.dataset.k === "drift") tn.drift = i.value / 100;
    else tn.tolerance = +i.value;
    show();
  }));
  pop.querySelector(".twchk input").onchange = (e) => { tn.keepChromatic = e.target.checked; };
  pop.querySelector(".no").onclick = () => pop.remove();
  pop.querySelector(".go").onclick = () => {
    const scale = tuneScale(twKey(c));
    let moved = 0;
    const notes = twNotes(c).map((n) => {
      if (tw.sel.size && !tw.sel.has(n.i)) return n;
      if (n.end - n.start < 0.08) return n;
      const target = tuneTarget(n.center, scale), dev = target - n.center;
      if (Math.abs(dev) * 100 <= tn.tolerance && !tw.sel.size) return { ...n, drift: tn.drift > 0 ? tn.drift : n.drift };
      moved++;
      return { ...n, shift: dev * tn.strength, drift: tn.drift };
    });
    twSave(c, notes);
    toast(moved ? `${moved} note${moved === 1 ? "" : "s"} put into ${scale ? keyLabel(twKey(c)) : "tune"}.` : "Every note is in tune already.");
    pop.remove();
  };
  show();
  tw.el.append(pop);
  const r = anchor.getBoundingClientRect(), wr = tw.el.getBoundingClientRect();
  pop.style.right = Math.max(8, wr.right - r.right) + "px";
  pop.style.top = r.bottom - wr.top + 6 + "px";
}

// --- geometry ---------------------------------------------------------------------------------

function twRange(c) {
  if (tw.lo != null) return { lo: tw.lo, hi: tw.hi };
  const ns = twNotes(c);
  if (!ns.length) return { lo: 52, hi: 76 };
  const ps = ns.flatMap((n) => [n.center, n.center + n.shift]);
  let lo = Math.floor(Math.min(...ps)) - 4, hi = Math.ceil(Math.max(...ps)) + 4;
  if (hi - lo < 18) { const m = Math.round((lo + hi) / 2); lo = m - 9; hi = m + 9; }
  return { lo, hi };
}

function twGeom(c, w, h) {
  const { a, b } = tw.view, { lo, hi } = twRange(c), top = TW_RULER, H = h - top;
  const rowH = H / (hi - lo + 1);
  return {
    x: (t) => TW_KEYS + (t - a) / (b - a) * (w - TW_KEYS),
    t: (px) => a + (px - TW_KEYS) / (w - TW_KEYS) * (b - a),
    y: (m) => top + (hi - m + 0.5) * rowH, m: (py) => hi + 0.5 - (py - top) / rowH, rowH, lo, hi, top,
  };
}

// --- drawing ---------------------------------------------------------------------------------

function twDraw() {
  const c = twClip(), cv = tw.el?.querySelector(".twcv");
  if (!c || !cv || tw.el.hidden) return;
  const w = cv.clientWidth, h = cv.clientHeight;
  if (!w || !h) return;
  const ctx = sizeCanvas(cv, w, h, false), g = twGeom(c, w, h);
  ctx.fillStyle = "#0d0c11";
  ctx.fillRect(0, 0, w, h);
  const key = twKey(c), scale = tuneScale(key), root = key ? NOTE_NAMES_JS.indexOf(key.split(" ")[0]) : -1;
  // the rows: black keys darker, the key's notes lit, its root more
  for (let m = g.lo; m <= g.hi; m++) {
    const pc = ((m % 12) + 12) % 12, y = g.y(m) - g.rowH / 2, black = [1, 3, 6, 8, 10].includes(pc);
    const inKey = scale ? scale.has(pc) : !black;
    ctx.fillStyle = pc === root ? "#1d1a29" : inKey ? "#16141d" : "#101015";
    ctx.fillRect(TW_KEYS, y, w - TW_KEYS, g.rowH);
    ctx.fillStyle = pc === 0 ? "#2a2735" : "#18161f";
    ctx.fillRect(TW_KEYS, Math.round(y + g.rowH) - 1, w - TW_KEYS, 1);
    // the keyboard
    ctx.fillStyle = black ? "#1f1d26" : "#d9d4e2";
    ctx.fillRect(0, y + 0.5, TW_KEYS - 6, g.rowH - 1);
    if (scale && scale.has(pc)) { ctx.fillStyle = pc === root ? "#4fd6c6" : "#4fd6c666"; ctx.fillRect(TW_KEYS - 5, y + 1, 4, g.rowH - 2); }
    if (pc === 0 || g.rowH >= 14) {
      ctx.fillStyle = black ? "#b8b1c6" : "#2a2732";
      ctx.font = `${pc === 0 ? 800 : 600} ${Math.min(11, Math.max(8, g.rowH - 4))}px system-ui, sans-serif`;
      ctx.fillText(midiName(m), 5, y + g.rowH / 2 + 4);
    }
  }
  // the clip's stretch of the file (outside it: dimmed)
  const o = c.offset || 0, L = ceSrcLen(c);
  ctx.fillStyle = "rgba(0,0,0,0.45)";
  ctx.fillRect(TW_KEYS, g.top, Math.max(0, g.x(o) - TW_KEYS), h);
  ctx.fillRect(Math.max(TW_KEYS, g.x(o + L)), g.top, w, h);
  twRuler(ctx, c, g, w);
  if (!tw.pitch) {
    ctx.fillStyle = "#b9b2c4";
    ctx.font = "600 14px system-ui, sans-serif";
    ctx.fillText(tw.error || (tw.loading ? "Listening to the voice…" : ""), TW_KEYS + 24, h / 2);
    return;
  }
  const p = tw.pitch, notes = twNotes(c), col = tColor(song.view.tracks.find((t) => t.id === c.track));
  const frameAt = (t) => Math.round((t - p.start) / p.hop);
  ctx.save();
  ctx.beginPath(); ctx.rect(TW_KEYS, g.top, w - TW_KEYS, h - g.top); ctx.clip();
  for (const n of notes) {
    const x0 = g.x(n.start), x1 = g.x(n.end);
    if (x1 < TW_KEYS || x0 > w) continue;
    const sel = tw.sel.has(n.i), now = n.center + n.shift, moved = Math.abs(n.shift) > 1e-4;
    const target = tuneTarget(n.center, scale), off = Math.abs(target - now) * 100 > tn.tolerance && n.end - n.start >= 0.08;
    // where it was sung
    if (moved) {
      ctx.strokeStyle = "#ffffff38"; ctx.setLineDash([3, 3]); ctx.lineWidth = 1;
      ctx.beginPath(); ctx.roundRect(x0, g.y(n.center) - g.rowH * 0.38, Math.max(2, x1 - x0), g.rowH * 0.76, 6); ctx.stroke();
      ctx.beginPath(); ctx.moveTo((x0 + x1) / 2, g.y(n.center)); ctx.lineTo((x0 + x1) / 2, g.y(now)); ctx.stroke();
      ctx.setLineDash([]);
    }
    // the blob: its outline is the voice's loudness (louder with a level move)
    const yc = g.y(now), half = g.rowH * 0.62, lvl = 10 ** (n.gain / 40);
    const f0 = frameAt(n.start), f1 = frameAt(n.end), steps = Math.max(3, Math.min(120, Math.round((x1 - x0) / 3)));
    const amp = (u) => { const f = Math.round(f0 + u * (f1 - f0)); return Math.min(1.25, (0.28 + 0.72 * Math.min(1, (p.rms[f] || 0) * 1.6)) * lvl); };
    ctx.beginPath();
    for (let s = 0; s <= steps; s++) { const u = s / steps, e = Math.sin(Math.PI * Math.min(1, u * 8, (1 - u) * 8) / 2); ctx.lineTo(x0 + u * (x1 - x0), yc - half * amp(u) * e); }
    for (let s = steps; s >= 0; s--) { const u = s / steps, e = Math.sin(Math.PI * Math.min(1, u * 8, (1 - u) * 8) / 2); ctx.lineTo(x0 + u * (x1 - x0), yc + half * amp(u) * e); }
    ctx.closePath();
    const base = sel ? "#ffffff" : col;
    const gr = ctx.createLinearGradient(0, yc - half, 0, yc + half);
    gr.addColorStop(0, base + (sel ? "f0" : "d0")); gr.addColorStop(1, base + (sel ? "b0" : "70"));
    ctx.fillStyle = gr;
    if (sel) { ctx.shadowColor = col; ctx.shadowBlur = 14; }
    ctx.fill(); ctx.shadowBlur = 0;
    // off the key: an amber edge (in tune: none)
    ctx.strokeStyle = sel ? col : tw.hover === n.i ? "#ffffffc0" : off ? "#ffb347" : "#00000070";
    ctx.lineWidth = sel ? 2 : off ? 1.6 : 1;
    ctx.stroke();
    // its centre, as a short line (where it's heard)
    if (x1 - x0 > 10) { ctx.fillStyle = sel ? "#0d0c11aa" : "#ffffff90"; ctx.fillRect(x0 + 4, Math.round(yc) - 1, x1 - x0 - 8, 2); }
    // a level move, written on it
    if (Math.abs(n.gain) > 0.05 && x1 - x0 > 34) {
      ctx.fillStyle = sel ? "#0d0c11" : "#fff"; ctx.font = "700 9px system-ui, sans-serif";
      ctx.fillText(`${n.gain > 0 ? "+" : ""}${n.gain.toFixed(1)}`, x0 + 5, yc - 4);
    }
  }
  // the pitch line, as it will sound (moves, drift and vibrato included)
  ctx.strokeStyle = "#ffffffe0"; ctx.lineWidth = 1.5; ctx.beginPath();
  let pen = false, ni = 0;
  const fa = Math.max(0, frameAt(tw.view.a)), fb = Math.min(p.midi.length - 1, frameAt(tw.view.b));
  const stepF = Math.max(1, Math.floor((fb - fa) / (w - TW_KEYS)));
  for (let f = fa; f <= fb; f += stepF) {
    const m = p.midi[f], t = p.start + f * p.hop;
    while (ni < notes.length && notes[ni].end < t) ni++;
    const n = notes[ni] && notes[ni].start <= t ? notes[ni] : null;
    if (m == null) { pen = false; continue; }
    let v = m;
    if (n) { const slow = twSlow(f); v = m + n.shift + n.drift * (n.center - slow) + (n.mod - 1) * (m - slow); }
    const x = g.x(t), y = g.y(v);
    pen ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
    pen = true;
  }
  ctx.stroke();
  ctx.restore();
  // the selection box
  if (tw.band) {
    const b = tw.band;
    ctx.fillStyle = "rgba(79,214,198,0.12)"; ctx.strokeStyle = "#4fd6c6";
    ctx.fillRect(Math.min(b.x0, b.x1), Math.min(b.y0, b.y1), Math.abs(b.x1 - b.x0), Math.abs(b.y1 - b.y0));
    ctx.strokeRect(Math.min(b.x0, b.x1), Math.min(b.y0, b.y1), Math.abs(b.x1 - b.x0), Math.abs(b.y1 - b.y0));
  }
  // the play cursor, and the playhead while it plays
  if (tw.cursor != null) { const x = g.x(tw.cursor); if (x >= TW_KEYS) { ctx.fillStyle = "#4fd6c6"; ctx.fillRect(Math.round(x), g.top, 1, h); } }
  const ph = twPlayhead(c);
  if (ph != null) {
    const x = g.x(ph);
    if (x >= TW_KEYS && x <= w) { ctx.fillStyle = "#fff"; ctx.fillRect(Math.round(x) - 0.5, 0, 2, h); }
  }
  // what's under the mouse
  if (tw.hoverInfo) {
    const { x, y, text } = tw.hoverInfo;
    ctx.font = "700 11px system-ui, sans-serif";
    const tw2 = ctx.measureText(text).width + 12, tx = Math.min(w - tw2 - 4, x + 12), ty = Math.max(g.top + 2, y - 26);
    ctx.fillStyle = "rgba(8,8,12,0.92)"; ctx.fillRect(tx, ty, tw2, 19);
    ctx.fillStyle = "#fff"; ctx.fillText(text, tx + 6, ty + 13);
  }
}

// the ruler: seconds of the clip, the cursor's triangle
function twRuler(ctx, c, g, w) {
  ctx.fillStyle = "#17151d"; ctx.fillRect(0, 0, w, TW_RULER);
  ctx.fillStyle = "#26232e"; ctx.fillRect(0, TW_RULER - 1, w, 1);
  const o = c.offset || 0, span = tw.view.b - tw.view.a;
  const step = [0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10, 30].find((s) => s / span * (w - TW_KEYS) > 70) || 60;
  ctx.font = "600 9.5px system-ui, sans-serif";
  for (let t = Math.ceil((tw.view.a - o) / step) * step; t + o <= tw.view.b; t += step) {
    const x = g.x(t + o);
    if (x < TW_KEYS) continue;
    ctx.fillStyle = "#4a4655"; ctx.fillRect(Math.round(x), TW_RULER - 7, 1, 7);
    ctx.fillStyle = "#9a94a8";
    ctx.fillText(t >= 60 ? `${Math.floor(t / 60)}:${(t % 60).toFixed(step < 1 ? 1 : 0).padStart(step < 1 ? 4 : 2, "0")}` : `${+t.toFixed(2)} s`, x + 3, 12);
  }
  if (tw.cursor != null) {
    const x = g.x(tw.cursor);
    if (x >= TW_KEYS) { ctx.fillStyle = "#4fd6c6"; ctx.beginPath(); ctx.moveTo(x - 5, 0); ctx.lineTo(x + 5, 0); ctx.lineTo(x, 8); ctx.closePath(); ctx.fill(); }
  }
  ctx.fillStyle = "#0d0c11"; ctx.fillRect(0, 0, TW_KEYS, TW_RULER);
}

// where the song's playhead is, in the file (null when it isn't playing this clip)
function twPlayhead(c) {
  if (!song.st || !song.st.playing) return null;
  const t = secsBetween(c.start, curPos());
  return t >= 0 && t <= c.len ? clipSrcAt(c, t) : null;
}

// a file time as a song position (beats)
const twBeatOf = (c, src) => s2b(b2s(c.start) + Math.max(0, (src - (c.offset || 0)) / clipRatio(c)));

async function twPlay() {
  const c = twClip();
  if (!c) return;
  if (song.st && song.st.playing) { await api.daw_stop(); twTick(); return; }
  const at = twBeatOf(c, tw.cursor ?? c.offset ?? 0);
  await api.daw_locate(at);
  await api.daw_play(at);
  twTick();
}

// while it plays: the playhead moves, the view follows
function twTick() {
  cancelAnimationFrame(tw.raf);
  tw.ticking = true;
  const step = () => {
    const c = twClip();
    if (!c || tw.el.hidden) { tw.ticking = false; return; }
    const ph = twPlayhead(c);
    tw.el.querySelector(".twplay").classList.toggle("on", !!(song.st && song.st.playing));
    if (ph != null && tw.follow && !tw.drag) {
      const span = tw.view.b - tw.view.a;
      if (ph > tw.view.b - span * 0.1 || ph < tw.view.a) { tw.view = { a: Math.max(0, ph - span * 0.1), b: Math.max(0, ph - span * 0.1) + span }; }
    }
    twDraw();
    if (song.st && song.st.playing) tw.raf = requestAnimationFrame(step);
    else tw.ticking = false;
  };
  tw.raf = requestAnimationFrame(step);
}

// the slow line through the pitch (150 ms): drift is its distance from the centre, vibrato the rest
function twSlow(f) {
  const p = tw.pitch, w = Math.round(0.075 / p.hop);
  if (!tw.slow) {
    const out = new Float32Array(p.midi.length);
    for (let k = 0; k < p.midi.length; k++) {
      let s = 0, n = 0;
      for (let j = Math.max(0, k - w); j <= Math.min(p.midi.length - 1, k + w); j++) if (p.midi[j] != null) { s += p.midi[j]; n++; }
      out[k] = n ? s / n : NaN;
    }
    tw.slow = out;
  }
  const v = tw.slow[f];
  return Number.isFinite(v) ? v : p.midi[f];
}

// --- hearing a note --------------------------------------------------------------------------

function twAudition(c, n) {
  if (!api.tune_audition) return;
  const sig = `${n.i}|${n.shift.toFixed(3)}|${n.mod.toFixed(2)}|${n.drift.toFixed(2)}|${n.gain.toFixed(1)}`;
  const now = performance.now();
  if (tw.lastAud && tw.lastAud.sig === sig && now - tw.lastAud.at < 400) return;
  tw.lastAud = { sig, at: now };
  api.tune_audition(c.id, n.start, n.end, n.center, n.shift, n.drift, n.mod, n.gain);
}

// --- the mouse and the keys ---------------------------------------------------------------------

function twHit(c, px, py, w, h) {
  const g = twGeom(c, w, h);
  let best = null, bd = 1e9;
  for (const n of twNotes(c)) {
    if (px < g.x(n.start) - 2 || px > g.x(n.end) + 2) continue;
    const d = Math.abs(py - g.y(n.center + n.shift));
    if (d <= Math.max(6, g.rowH * 0.6) && d < bd) { bd = d; best = n; }
  }
  return best;
}

function twSnapTo(to, scale, alt) {
  if (alt || tw.snap === "off") return to;
  if (tw.snap === "scale" && scale) {
    let best = Math.round(to), bd = 99;
    for (let m = Math.floor(to) - 2; m <= Math.ceil(to) + 2; m++) if (scale.has(((m % 12) + 12) % 12) && Math.abs(m - to) < bd) { best = m; bd = Math.abs(m - to); }
    return best;
  }
  return Math.round(to);
}

function twCanvas() {
  const cv = tw.el.querySelector(".twcv");
  const at = (e) => { const r = cv.getBoundingClientRect(); return { px: e.clientX - r.left, py: e.clientY - r.top, w: r.width, h: r.height }; };
  cv.addEventListener("pointermove", (e) => {
    const c = twClip();
    if (!c || tw.drag || tw.band || !tw.pitch) return;
    const q = at(e), n = q.py > TW_RULER ? twHit(c, q.px, q.py, q.w, q.h) : null;
    cv.style.cursor = q.py <= TW_RULER ? "text" : n ? (tw.tool === "pitch" ? "ns-resize" : "row-resize") : "crosshair";
    tw.hover = n ? n.i : -1;
    if (n) {
      const now = n.center + n.shift, ct = Math.round((now - Math.round(now)) * 100);
      const extra = tw.tool === "mod" ? ` · vibrato ${Math.round(n.mod * 100)}%` : tw.tool === "drift" ? ` · drift out ${Math.round(n.drift * 100)}%` : tw.tool === "gain" ? ` · ${n.gain >= 0 ? "+" : ""}${n.gain.toFixed(1)} dB` : "";
      tw.hoverInfo = { x: q.px, y: q.py, text: `${midiName(now)} ${ct >= 0 ? "+" : ""}${ct} ct${extra}` };
    } else tw.hoverInfo = null;
    twDraw();
  });
  cv.addEventListener("pointerleave", () => { tw.hover = -1; tw.hoverInfo = null; twDraw(); });
  cv.addEventListener("dblclick", (e) => {
    const c = twClip(), q = at(e), n = c && q.py > TW_RULER && twHit(c, q.px, q.py, q.w, q.h);
    if (!n) return;
    const scale = tuneScale(twKey(c));
    const notes = twNotes(c).map((x) => {
      if (x.i !== n.i && !(tw.sel.has(n.i) && tw.sel.has(x.i))) return x;
      if (tw.tool === "pitch") return { ...x, shift: tuneTarget(x.center, scale) - x.center };
      if (tw.tool === "mod") return { ...x, mod: 1 };
      if (tw.tool === "drift") return { ...x, drift: x.drift > 0.5 ? 0 : 1 };
      return { ...x, gain: 0 };
    });
    twSave(c, notes);
    twAudition(c, notes.find((x) => x.i === n.i));
  });
  cv.addEventListener("pointerdown", (e) => {
    const c = twClip();
    if (!c || e.button !== 0) return;
    const q = at(e);
    cv.setPointerCapture(e.pointerId);
    // the ruler: put the play cursor there (dragging scrubs it)
    if (q.py <= TW_RULER) {
      const g = twGeom(c, q.w, q.h);
      const place = (px) => { tw.cursor = Math.max(0, g.t(Math.max(TW_KEYS, px))); twDraw(); };
      place(q.px);
      const move = (ev) => place(at(ev).px);
      const up = async () => {
        cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up);
        if (song.st && song.st.playing) { const b = twBeatOf(c, tw.cursor); await api.daw_locate(b); await api.daw_play(b); twTick(); }
      };
      cv.addEventListener("pointermove", move); cv.addEventListener("pointerup", up);
      return;
    }
    if (!tw.pitch) return;
    const n = twHit(c, q.px, q.py, q.w, q.h), g = twGeom(c, q.w, q.h);
    if (!n) {
      // empty space: the cursor goes here, and a box selects
      tw.cursor = Math.max(0, g.t(Math.max(TW_KEYS, q.px)));
      if (!e.shiftKey) tw.sel.clear();
      tw.band = { x0: q.px, y0: q.py, x1: q.px, y1: q.py };
      const keep = new Set(tw.sel);
      const move = (ev) => {
        const r = at(ev);
        tw.band.x1 = r.px; tw.band.y1 = r.py;
        const [xa, xb] = [Math.min(tw.band.x0, r.px), Math.max(tw.band.x0, r.px)], [ya, yb] = [Math.min(tw.band.y0, r.py), Math.max(tw.band.y0, r.py)];
        tw.sel = new Set(keep);
        for (const x of twNotes(c)) { const y = g.y(x.center + x.shift); if (g.x(x.end) > xa && g.x(x.start) < xb && y > ya && y < yb) tw.sel.add(x.i); }
        twDraw();
      };
      const up = () => { cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up); tw.band = null; twDraw(); twInspect(); };
      cv.addEventListener("pointermove", move); cv.addEventListener("pointerup", up);
      twDraw();
      return;
    }
    if (e.shiftKey) { tw.sel.has(n.i) ? tw.sel.delete(n.i) : tw.sel.add(n.i); twDraw(); twInspect(); return; }
    if (!tw.sel.has(n.i)) { tw.sel.clear(); tw.sel.add(n.i); }
    twInspect();
    // you hear it as soon as you press it
    twAudition(c, n);
    const start = twNotes(c), scale = tuneScale(twKey(c)), m0 = g.m(q.py), y0 = q.py;
    tw.drag = { moved: false };
    const move = (ev) => {
      const r = at(ev);
      if (!tw.drag.moved && Math.abs(r.py - y0) < 3) return;
      tw.drag.moved = true;
      const dy = (y0 - r.py) / 100; // per 100 px up
      const notes = start.map((x) => {
        if (!tw.sel.has(x.i)) return x;
        if (tw.tool === "pitch") return { ...x, shift: twSnapTo(x.center + x.shift + (g.m(r.py) - m0), scale, ev.altKey) - x.center };
        if (tw.tool === "mod") return { ...x, mod: Math.max(0, Math.min(2, x.mod + dy)) };
        if (tw.tool === "drift") return { ...x, drift: Math.max(0, Math.min(1, x.drift + dy)) };
        return { ...x, gain: Math.max(-24, Math.min(12, x.gain + dy * 12)) };
      });
      twSave(c, notes);
      const me = notes.find((x) => x.i === n.i);
      const now = me.center + me.shift;
      tw.hoverInfo = { x: r.px, y: r.py, text: tw.tool === "pitch" ? `${midiName(now)} ${Math.round((now - Math.round(now)) * 100)} ct`
        : tw.tool === "mod" ? `vibrato ${Math.round(me.mod * 100)}%` : tw.tool === "drift" ? `drift out ${Math.round(me.drift * 100)}%` : `${me.gain >= 0 ? "+" : ""}${me.gain.toFixed(1)} dB` };
      // a new step: hear it there (the pitch tool on every semitone it reaches; the others as they settle)
      if (tw.tool === "pitch") twAudition(c, me);
      else { clearTimeout(tw.audT); tw.audT = setTimeout(() => twAudition(c, me), 140); }
    };
    const up = () => {
      cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up);
      tw.drag = null; tw.hoverInfo = null;
      twDraw(); twInspect();
    };
    cv.addEventListener("pointermove", move); cv.addEventListener("pointerup", up);
  });
  cv.addEventListener("wheel", (e) => {
    const c = twClip();
    if (!c) return;
    e.preventDefault();
    const r = cv.getBoundingClientRect();
    if (e.altKey) { // the pitch: zoom around the mouse
      const g = twGeom(c, r.width, r.height), at2 = g.m(e.clientY - r.top), { lo, hi } = twRange(c);
      const span = Math.max(8, Math.min(60, (hi - lo) * Math.exp(e.deltaY * 0.002)));
      const f = (at2 - lo) / Math.max(1, hi - lo);
      tw.lo = Math.round(at2 - f * span); tw.hi = Math.round(tw.lo + span);
      twDraw(); return;
    }
    const dur = ceFileDur(c), { a, b } = tw.view, span = b - a;
    const frac = Math.max(0, (e.clientX - r.left - TW_KEYS) / (r.width - TW_KEYS));
    if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
      const d = (e.shiftKey ? e.deltaY : e.deltaX) / r.width * span;
      const na = Math.max(0, Math.min(dur - span, a + d));
      tw.view = { a: na, b: na + span };
    } else {
      const at2 = a + frac * span, ns = Math.max(0.05, Math.min(dur, span * Math.exp(e.deltaY * 0.002)));
      const na = Math.max(0, Math.min(dur - ns, at2 - frac * ns));
      tw.view = { a: na, b: na + ns };
    }
    twDraw();
  }, { passive: false });
}

function twKeys() {
  addEventListener("keydown", (e) => {
    if (!tw.el || tw.el.hidden || !tw.active || e.target.closest("input, select, textarea")) return;
    const c = twClip();
    if (!c) return;
    const k = e.key, stop = () => { e.preventDefault(); e.stopPropagation(); };
    if (k === " ") { stop(); twPlay(); return; }
    if (k === "Escape") { stop(); if (tw.sel.size) { tw.sel.clear(); twDraw(); twInspect(); } else closeTune(); return; }
    const tool = { 1: "pitch", 2: "mod", 3: "drift", 4: "gain" }[k];
    if (tool && !e.ctrlKey && !e.metaKey) { stop(); tw.tool = tool; twChrome(); return; }
    if (!tw.pitch) return;
    const notes = twNotes(c), scale = tuneScale(twKey(c));
    if ((e.ctrlKey || e.metaKey) && k.toLowerCase() === "a") { stop(); tw.sel = new Set(notes.map((n) => n.i)); twDraw(); twInspect(); return; }
    if (!tw.sel.size) return;
    if (k === "ArrowUp" || k === "ArrowDown") {
      stop();
      const dir = k === "ArrowUp" ? 1 : -1;
      const moved = notes.map((n) => {
        if (!tw.sel.has(n.i)) return n;
        let to = Math.round(n.center + n.shift) + dir;
        if (tw.snap === "scale" && scale && !e.altKey) while (!scale.has(((to % 12) + 12) % 12)) to += dir;
        return { ...n, shift: to - n.center };
      });
      twSave(c, moved);
      twAudition(c, moved.find((n) => tw.sel.has(n.i)));
    } else if (k === "Delete" || k === "Backspace") {
      stop();
      twSave(c, notes.map((n) => (tw.sel.has(n.i) ? { ...n, shift: 0, drift: 0, mod: 1, gain: 0 } : n)));
    }
  }, true);
}

// the song changed (an undo, the partner): redraw if it's this clip
function twSongChanged() {
  if (!tw.el || tw.el.hidden) return;
  const c = twClip();
  if (!c) { closeTune(); return; }
  twLoad(c);
  twChrome(); twDraw(); twInspect();
  if (song.st && song.st.playing) twTick();
}

// playing started from elsewhere (the transport, the partner): the playhead runs here too
setInterval(() => {
  if (tw.el && !tw.el.hidden && song.st && song.st.playing && !tw.ticking) twTick();
}, 250);
