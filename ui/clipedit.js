// The clip editor: double-click an audio clip (FL's audio clip settings plus a
// bit of Edison). Everything in it changes the clip, never the file: it's all
// in the shared song, so your partner hears the same, and undo takes it back.
//
// The waveform shows the whole file, the clip's stretch of it lit up: drag its
// edges to trim, drag inside to slide the sound under the clip (it stays where
// it is in the song). Tabs:
// - Clip: volume and normalize, fades, backwards.
// - Pitch & speed: transpose and speed, either keeping the pitch (a stretched
//   copy, made in the background) or as tape (both together, as it plays);
//   the tempo found in the sound, fit it to the song, or to a number of bars.
// - Chop: cuts at the hits or on the grid (or click your own), then into clips.

const ce = { id: null, tab: "clip", marks: [], sens: 0.5, bpm: null, view: null, drag: null, hover: null };
const CE_EDGE = 7;

const ceClip = () => (ce.id && song.view ? song.view.clips.find((c) => c.id === ce.id) || null : null);
const ceSrcLen = (c) => c.len * clipRatio(c); // file seconds the clip covers
function ceFileDur(c) {
  const f = song.view.files[c.file];
  return f && f.frames ? f.frames / f.sr : (c.offset || 0) + ceSrcLen(c);
}

function openClipEditor(id) {
  const c = song.view && song.view.clips.find((k) => k.id === id);
  if (!c || c.pattern) return;
  ce.id = id;
  ce.marks = [];
  ce.bpm = null;
  ce.drag = null;
  const o = c.offset || 0, L = ceSrcLen(c), dur = ceFileDur(c), pad = Math.max(0.25, L * 0.12);
  ce.view = { a: Math.max(0, o - pad), b: Math.min(dur, o + L + pad) };
  if (ce.view.b - ce.view.a < 0.05) ce.view = { a: 0, b: Math.max(0.05, dur) };
  $("clipEd").hidden = false;
  ceRender();
}

function closeClipEditor() {
  $("clipEd").hidden = true;
  ce.id = null;
  hideAutoValue?.();
}

// a change to the clip: shown at once, sent along (a run of them is one undo step)
function ceEdit(fields) {
  const c = ceClip();
  if (!c) return;
  Object.assign(c, fields);
  throttledEdit("clip", c.id, fields);
  songDraw();
  ceRender();
}

// speed or pitch: the clip keeps covering the same stretch of the file, so
// its length on the timeline follows
function ceSetRate(fields) {
  const c = ceClip();
  if (!c) return;
  const L = ceSrcLen(c), next = { ...c, ...fields }, len = L / clipRatio(next);
  ceEdit({ ...fields, len, fade_in: Math.min(c.fade_in || 0, len / 2), fade_out: Math.min(c.fade_out || 0, len / 2) });
}

// --- the window -----------------------------------------------------------------

function ceRender() {
  const c = ceClip();
  if (!c) { if (!$("clipEd").hidden) closeClipEditor(); return; }
  const t = song.view.tracks.find((x) => x.id === c.track), f = song.view.files[c.file] || {};
  $("ceTitle").textContent = (t ? t.name : "Clip") + clipBadges(c);
  $("ceSub").textContent = `${f.name || "Audio"} · ${ceTime(ceFileDur(c))} file · ${ceTime(c.len)} in the song`;
  for (const b of $("ceTabs").querySelectorAll("button")) b.classList.toggle("on", b.dataset.v === ce.tab);
  // the Tune tab has its piano roll where the waveform is
  const tuning = ce.tab === "tune";
  $("ceWave").hidden = tuning; $("tuneCv").hidden = !tuning;
  document.querySelector("#clipEd .cehint").hidden = tuning;
  $("cePlay").classList.toggle("on", !!(song.st && song.st.playing));
  const body = $("ceBody");
  // rebuilt, unless one of its sliders is being dragged (then just the values)
  if (body.dataset.tab !== ce.tab || !(ce.busy || song.headBusy)) {
    body.dataset.tab = ce.tab;
    body.innerHTML = "";
    body.append(ce.tab === "clip" ? ceClipTab(c) : ce.tab === "pitch" ? cePitchTab(c) : ce.tab === "chop" ? ceChopTab(c)
      : ce.tab === "tune" ? tuneTab(c) : ceStemsTab(c));
  } else {
    ceRefresh(c);
  }
  ceDraw();
}

// small live bits (values next to the controls) without rebuilding the tab
function ceRefresh(c) {
  for (const el of $("ceBody").querySelectorAll("[data-live]")) el.textContent = ceLive(c, el.dataset.live);
}
function ceLive(c, what) {
  const sp = c.speed ?? 1, p = c.pitch || 0;
  if (what === "gain") return fmtDb(c.gain ?? 1);
  if (what === "fin") return ceMs((c.fade_in || 0) * 1000);
  if (what === "fout") return ceMs((c.fade_out || 0) * 1000);
  if (what === "speed") return `${Math.round(sp * 1000) / 10}%`;
  if (what === "pitch") return `${p > 0 ? "+" : ""}${p.toFixed(2)} st`;
  if (what === "len") return `${ceTime(c.len)} · ${(c.len / spb()).toFixed(2)} beats`;
  if (what === "marks") return ce.marks.length ? `${ce.marks.length} cut${ce.marks.length === 1 ? "" : "s"} → ${ce.marks.length + 1} clips` : "No cuts yet";
  if (what === "state") return stretching(c) ? "Stretching… (the clip is silent until it's ready)" : "";
  return "";
}

function ceRow(label, ...els) {
  const row = document.createElement("div");
  row.className = "cerow";
  const l = document.createElement("label");
  l.textContent = label;
  row.append(l, ...els);
  return row;
}
function ceVal(what, c) {
  const s = document.createElement("span");
  s.className = "ceval";
  s.dataset.live = what;
  s.textContent = ceLive(c, what);
  return s;
}
function ceBtn(text, title, fn, cls = "btn") {
  const b = document.createElement("button");
  b.className = cls;
  b.textContent = text;
  if (title) b.title = title;
  b.onclick = fn;
  return b;
}
// a slider 0..1 (`to`/`from` map it to the value); a run of moves is one edit
function ceRange(pos, onInput, title) {
  const r = document.createElement("input");
  r.type = "range";
  r.className = "cerange";
  r.min = 0; r.max = 1000; r.value = Math.round(pos * 1000);
  if (title) r.title = title;
  r.oninput = () => { ce.busy = true; onInput(r.value / 1000); };
  r.onchange = () => { ce.busy = false; ceRender(); };
  return r;
}
function ceSeg(options, cur, fn) {
  const seg = document.createElement("div");
  seg.className = "seg";
  for (const [v, label, title] of options) {
    const b = document.createElement("button");
    b.textContent = label;
    b.dataset.v = v;
    if (title) b.title = title;
    b.classList.toggle("on", v === cur);
    b.onclick = () => fn(v);
    seg.append(b);
  }
  return seg;
}

function ceClipTab(c) {
  const box = document.createElement("div");
  const gain = c.gain ?? 1;
  box.append(ceRow("Volume", hslider(gain, (g) => ceEdit({ gain: g }), `Clip volume: ${fmtDb(gain)}`), ceVal("gain", c),
    ceBtn("Normalize", "As loud as it goes without clipping", async () => {
      const r = await api.daw_analyze(c.id, "peak");
      if (!r || r.error) { toast(r ? r.error : "Couldn't look at the audio", true); return; }
      if (r.peak < 1e-5) { toast("That stretch of the file is silent."); return; }
      ceEdit({ gain: Math.min(32, 0.977 / r.peak) }); // -0.2 dB
    })));
  const maxFade = Math.min(c.len, 30);
  const fadePos = (v) => Math.sqrt(Math.min(1, v / maxFade)), fadeOf = (p) => p * p * maxFade;
  box.append(ceRow("Fade in", ceRange(fadePos(c.fade_in || 0), (p) => ceEdit({ fade_in: Math.min(fadeOf(p), c.len - (c.fade_out || 0)) })), ceVal("fin", c)));
  box.append(ceRow("Fade out", ceRange(fadePos(c.fade_out || 0), (p) => ceEdit({ fade_out: Math.min(fadeOf(p), c.len - (c.fade_in || 0)) })), ceVal("fout", c)));
  const rev = ceBtn(c.reverse ? "◀ Backwards (on)" : "◀ Backwards", "Play the clip's stretch of the file backwards", () => ceEdit({ reverse: !c.reverse }),
    "btn" + (c.reverse ? " teal" : ""));
  box.append(ceRow("", rev, ceBtn("Reset everything", "Back to the sound as it was recorded", () => ceSetRate({
    gain: 1, fade_in: 0, fade_out: 0, reverse: false, speed: 1, pitch: 0, stretch: false }))));
  return box;
}

function cePitchTab(c) {
  const box = document.createElement("div");
  box.append(ceRow("Mode", ceSeg([
    ["keep", "Keep pitch", "Speed and pitch apart (the sound is stretched in the background: a moment)"],
    ["tape", "Tape", "Faster is higher, like a record or a tape: instant, no artifacts"]],
  c.stretch ? "keep" : "tape", (v) => ceSetRate({ stretch: v === "keep" }))));
  const p = c.pitch || 0;
  const step = (d) => ceSetRate({ pitch: Math.max(-24, Math.min(24, Math.round(p) + d)) });
  const cents = ceRange((p - Math.round(p) + 0.5), (x) => ceSetRate({ pitch: Math.round(c.pitch || 0) + (x - 0.5) }), "Fine tune (cents)");
  box.append(ceRow("Transpose", ceBtn("−", "A semitone down", () => step(-1), "btn sq"), ceVal("pitch", c),
    ceBtn("+", "A semitone up", () => step(1), "btn sq"), ceBtn("−12", "An octave down", () => step(-12), "btn sq"),
    ceBtn("+12", "An octave up", () => step(12), "btn sq"), cents));
  // speed: 25%..400% on a log scale, 100% in the middle
  const sp = c.speed ?? 1, spPos = (Math.log2(sp) + 2) / 4;
  box.append(ceRow("Speed", ceRange(spPos, (x) => ceSetRate({ speed: Math.round(2 ** (x * 4 - 2) * 1000) / 1000 })), ceVal("speed", c),
    ceBtn("½×", "Half speed", () => ceSetRate({ speed: sp / 2 }), "btn sq"), ceBtn("1×", "As recorded", () => ceSetRate({ speed: 1 }), "btn sq"),
    ceBtn("2×", "Double speed", () => ceSetRate({ speed: sp * 2 }), "btn sq")));
  box.append(ceRow("Length", ceVal("len", c)));
  // tempo
  const bpmIn = document.createElement("input");
  bpmIn.type = "number"; bpmIn.min = 30; bpmIn.max = 300; bpmIn.step = 0.1; bpmIn.className = "cenum";
  bpmIn.placeholder = "BPM";
  if (ce.bpm) bpmIn.value = ce.bpm;
  bpmIn.onchange = () => { ce.bpm = +bpmIn.value || null; };
  const songBpm = song.view.meta.bpm;
  box.append(ceRow("Its tempo", bpmIn,
    ceBtn("Detect", "Find the tempo in the sound", async () => {
      const r = await api.daw_analyze(c.id, "tempo");
      if (!r || r.error) { toast(r ? r.error : "Couldn't look at the audio", true); return; }
      if (!r.bpm) { toast("No steady beat found: type its tempo in."); return; }
      // speed changes what we measured: the sound's own tempo is that over the speed
      ce.bpm = Math.round(r.bpm / clipRatio(c) * 10) / 10;
      bpmIn.value = ce.bpm;
    }),
    ceBtn(`Fit to the song (${Math.round(songBpm)} BPM)`, "Speed it up or down so its beat is the song's (keeping its pitch)", () => {
      if (!ce.bpm) { toast("Detect or type its tempo first."); return; }
      ceSetRate({ speed: songBpm / ce.bpm, stretch: true });
    }, "btn teal")));
  const bars = document.createElement("div");
  bars.className = "cebtns";
  for (const n of [1, 2, 4, 8, 16]) {
    bars.append(ceBtn(`${n} bar${n > 1 ? "s" : ""}`, `Stretch it to exactly ${n} bar${n > 1 ? "s" : ""} of the song`, () => {
      const want = n * bpb() * spb(); // seconds on the timeline
      ceSetRate({ speed: ceSrcLen(c) / want, stretch: true });
    }, "btn sq"));
  }
  box.append(ceRow("Make it last", bars));
  const st = document.createElement("p");
  st.className = "fine";
  st.dataset.live = "state";
  st.textContent = ceLive(c, "state");
  box.append(st);
  return box;
}

function ceChopTab(c) {
  const box = document.createElement("div");
  box.append(ceRow("Hits", ceRange(ce.sens, (x) => { ce.sens = x; }, "Sensitivity: further right finds softer hits"),
    ceBtn("Find the hits", "A cut where every hit starts", async () => {
      const r = await api.daw_analyze(c.id, "hits", ce.sens);
      if (!r || r.error) { toast(r ? r.error : "Couldn't look at the audio", true); return; }
      const o = c.offset || 0, L = ceSrcLen(c);
      ce.marks = r.hits.filter((h) => h > o + 0.01 && h < o + L - 0.01);
      if (!ce.marks.length) toast("No hits found: try further right on the slider.");
      ceRender();
    })));
  const grid = ceSeg([["bar", "Bar"], ["1", "Beat"], ["0.5", "1/8"], ["0.25", "1/16"]], null, (v) => {
    const step = v === "bar" ? bpb() : +v, b0 = c.start, b1 = c.start + c.len / spb();
    ce.marks = [];
    for (let b = Math.ceil((b0 + 1e-6) / step) * step; b < b1 - 1e-6; b += step) ce.marks.push(ceSrcOfBeat(c, b));
    ceRender();
  });
  box.append(ceRow("On the grid", grid));
  box.append(ceRow("", ceVal("marks", c), ceBtn("Clear the cuts", "", () => { ce.marks = []; ceRender(); }),
    ceBtn("Chop into clips", "Cut the clip at every mark", () => ceChop(c), "btn teal")));
  const hint = document.createElement("p");
  hint.className = "fine";
  hint.textContent = "Click the waveform to add a cut, drag one to move it, right-click it to take it away.";
  box.append(hint);
  return box;
}

// --- stems ------------------------------------------------------------------------

// the split's state (asked for while the tab is open)
const stemsUi = { set: "4", state: null, timer: null };

function ceStemsTab(c) {
  const box = document.createElement("div");
  box.className = "cestems";
  box.append(ceRow("Into", ceSeg([["4", "4 stems", "Drums, bass, vocals and the rest, each on its own track"],
    ["2", "Vocals + instrumental", "The voice on one track, everything else on another"]], stemsUi.set, (v) => { stemsUi.set = v; ceRender(); })));
  const st = stemsUi.state, live = document.createElement("div");
  live.className = "cestemlive";
  box.append(live);
  const p = document.createElement("p");
  p.className = "fine";
  p.textContent = "Demucs (Meta's stem splitter) works it out on this computer: nothing is sent anywhere. The new tracks go "
    + "under this one, each with a clip in the same place; this clip is muted (right-click it to hear it again). "
    + "Your partner gets the stems as files, like a recording.";
  box.append(p);
  stemsPaint(live, c, st);
  stemsWatch();
  return box;
}

function stemsPaint(live, c, st) {
  live.innerHTML = "";
  const bar = (frac, label) => {
    const w = document.createElement("div");
    w.className = "cebar";
    w.innerHTML = `<div class="exprog"><i style="width:${Math.round(frac * 100)}%"></i></div><span>${esc(label)}</span>`;
    return w;
  };
  if (!st) { live.textContent = "…"; return; }
  if (st.available === false) {
    live.innerHTML = `<p class="mnote">Stem splitting isn't in this build of Duet yet: the part that runs the model (ONNX Runtime) has no ready-made version for Intel Macs. It works on Apple Silicon Macs (M1 and newer).</p>`;
    return;
  }
  if (st.download && !st.download.error) {
    const d = st.download, mb = (x) => Math.round(x / 1e6);
    live.append(bar(d.total ? d.done / d.total : 0, `Downloading the stem model: ${mb(d.done)} of ${mb(d.total)} MB`));
    return;
  }
  if (st.job) {
    const j = st.job, mine = j.clip === c.id;
    const label = j.stage === "loading" ? "Getting ready…" : j.stage === "writing" ? "Writing the stems…"
      : `Splitting… ${Math.round((j.progress || 0) * 100)}%`;
    live.append(bar(j.stage === "splitting" ? j.progress || 0 : j.stage === "writing" ? 1 : 0, mine ? label : "Another clip is being split…"),
      ceBtn("Stop", "Stop splitting", () => api.stems_cancel()));
    return;
  }
  if (st.download && st.download.error) {
    const e = document.createElement("p");
    e.className = "mnote bad";
    e.textContent = st.download.error;
    live.append(e);
  }
  if (!st.model) {
    const mb = Math.round((st.size || 174e6) / 1e6);
    live.append(ceBtn(`Download the stem model (${mb} MB)`, "Once: it's kept on this computer", async () => {
      await api.stems_get_model(); stemsRefresh();
    }, "btn teal"));
    const n = document.createElement("span");
    n.className = "fine";
    n.textContent = "  Once, from Duet's releases on GitHub.";
    live.append(n);
    return;
  }
  live.append(ceBtn(stemsUi.set === "2" ? "Split into vocals + instrumental" : "Split into 4 stems", "", async () => {
    const r = await api.stems_split(c.id, stemsUi.set);
    if (r && r.error) toast(r.error, true);
    stemsRefresh();
  }, "btn teal"));
}

async function stemsRefresh() {
  stemsUi.state = await api.stems_state();
  const live = document.querySelector("#ceBody .cestemlive"), c = ceClip();
  if (live && c) stemsPaint(live, c, stemsUi.state);
}

// while the Stems tab is open (or a split runs): ask how it's going
function stemsWatch() {
  if (stemsUi.timer) return;
  stemsRefresh();
  stemsUi.timer = setInterval(() => {
    const open = !$("clipEd").hidden && ce.tab === "stems";
    if (!open && !(stemsUi.state && (stemsUi.state.job || stemsUi.state.download))) { clearInterval(stemsUi.timer); stemsUi.timer = null; return; }
    if (open) stemsRefresh();
  }, 400);
}

// --- cuts ------------------------------------------------------------------------

// timeline seconds from the clip's start <-> file seconds
function ceTimeOfSrc(c, src) {
  const r = clipRatio(c), o = c.offset || 0;
  return c.reverse ? (o + c.len * r - src) / r : (src - o) / r;
}
const ceSrcOfBeat = (c, beat) => clipSrcAt(c, (beat - c.start) * spb());

function ceChop(c) {
  const s = spb(), r = clipRatio(c), o = c.offset || 0;
  const ts = [...new Set(ce.marks.map((m) => ceTimeOfSrc(c, m)))].filter((t) => t > 0.005 && t < c.len - 0.005).sort((a, b) => a - b);
  if (!ts.length) { toast("Add some cuts first."); return; }
  const edges = [0, ...ts, c.len], ch = [], ids = [];
  for (let k = 0; k + 1 < edges.length; k++) {
    const a = edges[k], b = edges[k + 1];
    const piece = { start: c.start + a / s, len: b - a, offset: c.reverse ? o + (c.len - b) * r : o + a * r,
      fade_in: k === 0 ? Math.min(c.fade_in || 0, b - a) : 0, fade_out: k === edges.length - 2 ? Math.min(c.fade_out || 0, b - a) : 0 };
    if (k === 0) { ch.push(["clip", c.id, piece]); ids.push(c.id); }
    else { const id = newId("c"); ch.push(["clip", id, { ...clipFields(c), ...piece }]); ids.push(id); }
  }
  songEdit(ch);
  song.sel = new Set(ids);
  closeClipEditor();
  toast(`Chopped into ${ids.length} clips.`);
}

// --- the waveform -------------------------------------------------------------------

function ceDraw() {
  if (ce.tab === "tune") { tuneDraw(); return; }
  const c = ceClip(), cv = $("ceWave");
  if (!c || !cv || $("clipEd").hidden) return;
  const w = cv.clientWidth, h = cv.clientHeight;
  if (!w || !h) return;
  const ctx = sizeCanvas(cv, w, h), { a, b } = ce.view, x = (t) => (t - a) / (b - a) * w;
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = "#0e0d12";
  ctx.fillRect(0, 0, w, h);
  const o = c.offset || 0, L = ceSrcLen(c), col = tColor(song.view.tracks.find((t) => t.id === c.track));
  // the clip's stretch of the file
  ctx.fillStyle = col + "22";
  ctx.fillRect(x(o), 0, x(o + L) - x(o), h);
  // the file's waveform (dim outside the clip)
  const levels = song.peaks[c.file], f = song.view.files[c.file] || {};
  if (levels) {
    const sr = f.sr || 48000, secPerPx = (b - a) / w;
    let k = 0;
    while (k + 1 < levels.length && (512 << (k + 1)) <= secPerPx * sr) k++;
    const pk = levels[k], per = 512 << k, mid = h / 2, g = (h / 2 - 6) / 127;
    for (let px = 0; px < w; px++) {
      const t0 = a + px * secPerPx;
      const i0 = Math.max(0, Math.floor(t0 * sr / per)), i1 = Math.max(i0 + 1, Math.floor((t0 + secPerPx) * sr / per));
      let lo = 0, hi = 0;
      for (let i = i0; i < i1 && 2 * i + 1 < pk.length; i++) { lo = Math.min(lo, pk[2 * i]); hi = Math.max(hi, pk[2 * i + 1]); }
      const inside = t0 >= o && t0 <= o + L;
      ctx.fillStyle = inside ? col + "f0" : "#5a5466";
      ctx.fillRect(px, mid - hi * g, 1, Math.max(1, (hi - lo) * g));
    }
  } else if (f.here && !song.peakReq[c.file]) {
    song.peakReq[c.file] = true;
    api.daw_peaks(c.file).then((bb) => { if (bb) { song.peaks[c.file] = peakLevels(decodePeaks(bb)); ceDraw(); songDraw(); } else song.peakReq[c.file] = false; });
  }
  // edges, with handles
  ctx.fillStyle = col;
  for (const t of [o, o + L]) {
    ctx.fillRect(x(t) - 1, 0, 2, h);
    ctx.fillRect(x(t) - 5, 0, 10, 10);
  }
  // fades, as the clip plays them (from its start: at the file's end when backwards)
  const fin = (c.fade_in || 0) * clipRatio(c), fout = (c.fade_out || 0) * clipRatio(c);
  ctx.strokeStyle = "#ffffffa0";
  ctx.lineWidth = 1.5;
  const [startT, endT] = c.reverse ? [o + L, o] : [o, o + L], dir = c.reverse ? -1 : 1;
  if (fin > 0) { ctx.beginPath(); ctx.moveTo(x(startT), h); ctx.lineTo(x(startT + dir * fin), 12); ctx.stroke(); }
  if (fout > 0) { ctx.beginPath(); ctx.moveTo(x(endT - dir * fout), 12); ctx.lineTo(x(endT), h); ctx.stroke(); }
  // cuts
  for (const [i, m] of ce.marks.entries()) {
    ctx.fillStyle = ce.hover === i ? "#ffffff" : "#ffc857";
    ctx.fillRect(x(m) - 1, 0, 2, h);
    ctx.beginPath(); ctx.moveTo(x(m) - 5, h); ctx.lineTo(x(m) + 5, h); ctx.lineTo(x(m), h - 8); ctx.closePath(); ctx.fill();
  }
  // the playhead, while the song plays the clip
  if (song.st && song.st.playing) {
    const t = (curPos() - c.start) * spb();
    if (t >= 0 && t <= c.len) {
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(x(clipSrcAt(c, t)), 0, 1.5, h);
    }
  }
  if (c.reverse) {
    ctx.fillStyle = "#ffe2a3";
    ctx.font = "600 11px system-ui, sans-serif";
    ctx.fillText("◀ plays backwards", Math.max(4, x(o) + 6), h - 8);
  }
  // the time ruler along the top
  ctx.fillStyle = "#8a8494";
  ctx.font = "10px system-ui, sans-serif";
  const span = b - a, steps = [0.01, 0.05, 0.1, 0.5, 1, 5, 10, 30, 60, 300, 600];
  const st = steps.find((s) => span / s <= 12) || 1200;
  for (let t = Math.ceil(a / st) * st; t < b; t += st) ctx.fillText(ceTime(t), x(t) + 3, 10);
}

function ceHit(c, px, w) {
  const { a, b } = ce.view, xs = (t) => (t - a) / (b - a) * w, o = c.offset || 0, L = ceSrcLen(c);
  if (ce.tab === "chop") {
    const i = ce.marks.findIndex((m) => Math.abs(xs(m) - px) <= CE_EDGE);
    if (i >= 0) return { what: "mark", i };
  }
  if (Math.abs(xs(o) - px) <= CE_EDGE) return { what: "in" };
  if (Math.abs(xs(o + L) - px) <= CE_EDGE) return { what: "out" };
  const t = a + px / w * (b - a);
  if (t > o && t < o + L) return { what: ce.tab === "chop" ? "add" : "slip" };
  return null;
}

function ceSetup() {
  const cv = $("ceWave");
  tuneSetup();
  $("ceX").onclick = closeClipEditor;
  $("clipEd").onclick = (e) => e.target === $("clipEd") && closeClipEditor();
  for (const b of $("ceTabs").querySelectorAll("button")) b.onclick = () => { ce.tab = b.dataset.v; ceRender(); };
  $("cePlay").onclick = async () => {
    const c = ceClip();
    if (!c) return;
    if (song.st && song.st.playing) await api.daw_stop();
    else { await api.daw_locate(c.start); await api.daw_play(c.start); }
  };
  addEventListener("keydown", (e) => {
    if ($("clipEd").hidden) return;
    if (e.key === "Escape" && !(ce.tab === "tune" && tn.sel.size)) { e.preventDefault(); e.stopPropagation(); closeClipEditor(); }
  }, true);
  cv.addEventListener("pointermove", (e) => {
    const c = ceClip();
    if (!c || ce.drag) return;
    const r = cv.getBoundingClientRect(), h = ceHit(c, e.clientX - r.left, r.width);
    cv.style.cursor = !h ? "" : h.what === "mark" || h.what === "in" || h.what === "out" ? "ew-resize" : h.what === "add" ? "crosshair" : "grab";
    const hov = h && h.what === "mark" ? h.i : null;
    if (hov !== ce.hover) { ce.hover = hov; ceDraw(); }
  });
  cv.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const c = ceClip();
    if (!c) return;
    const r = cv.getBoundingClientRect(), h = ceHit(c, e.clientX - r.left, r.width);
    if (h && h.what === "mark") { ce.marks.splice(h.i, 1); ce.hover = null; ceRender(); }
  });
  cv.addEventListener("pointerdown", (e) => {
    const c = ceClip();
    if (!c || e.button !== 0) return;
    const r = cv.getBoundingClientRect(), w = r.width, h = ceHit(c, e.clientX - r.left, w);
    if (!h) return;
    cv.setPointerCapture(e.pointerId);
    const tAt = (ev) => ce.view.a + (ev.clientX - r.left) / w * (ce.view.b - ce.view.a);
    const dur = ceFileDur(c), o0 = c.offset || 0, L0 = ceSrcLen(c), r0 = clipRatio(c), t0 = tAt(e);
    if (h.what === "add") { ce.marks.push(t0); ce.marks.sort((x, y) => x - y); h.what = "mark"; h.i = ce.marks.indexOf(t0); ceRender(); }
    ce.drag = h;
    const move = (ev) => {
      const t = tAt(ev);
      if (h.what === "mark") {
        ce.marks[h.i] = Math.max(o0, Math.min(o0 + L0, t));
        ceDraw();
      } else if (h.what === "in") { // the clip stays where it is in the song; its length follows
        const o = Math.max(0, Math.min(o0 + L0 - 0.01, t));
        ceEdit({ offset: o, len: (o0 + L0 - o) / r0 });
      } else if (h.what === "out") {
        const end = Math.max(o0 + 0.01, Math.min(dur, t));
        ceEdit({ len: (end - o0) / r0 });
      } else if (h.what === "slip") { // the sound slides under the clip
        ceEdit({ offset: Math.max(0, Math.min(dur - L0, o0 + (t - t0))) });
      }
    };
    const up = () => {
      cv.removeEventListener("pointermove", move);
      cv.removeEventListener("pointerup", up);
      if (h.what === "mark") ce.marks.sort((x, y) => x - y);
      ce.drag = null;
      ceRender();
    };
    cv.addEventListener("pointermove", move);
    cv.addEventListener("pointerup", up);
  });
  // wheel: zoom around the pointer; with shift (or sideways): scroll
  cv.addEventListener("wheel", (e) => {
    const c = ceClip();
    if (!c) return;
    e.preventDefault();
    const r = cv.getBoundingClientRect(), dur = ceFileDur(c), { a, b } = ce.view, span = b - a;
    if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
      const d = (e.shiftKey ? e.deltaY : e.deltaX) / r.width * span;
      const na = Math.max(0, Math.min(dur - span, a + d));
      ce.view = { a: na, b: na + span };
    } else {
      const at = a + (e.clientX - r.left) / r.width * span, f = Math.exp(e.deltaY * 0.002);
      const ns = Math.max(0.02, Math.min(dur, span * f));
      const na = Math.max(0, Math.min(dur - ns, at - (at - a) * ns / span));
      ce.view = { a: na, b: na + ns };
    }
    ceDraw();
  }, { passive: false });
  addEventListener("resize", ceDraw);
  // the playhead moves while it plays
  const tick = () => { if (!$("clipEd").hidden && song.st && song.st.playing) ceDraw(); requestAnimationFrame(tick); };
  requestAnimationFrame(tick);
}

function ceMs(ms) { return ms < 1000 ? Math.round(ms) + " ms" : (ms / 1000).toFixed(2) + " s"; }
function ceTime(s) {
  if (s < 60) return (s < 10 ? s.toFixed(2) : s.toFixed(1)) + " s";
  const m = Math.floor(s / 60), r = s - 60 * m;
  return m >= 60 ? `${Math.floor(m / 60)}:${String(m % 60).padStart(2, "0")}:${String(Math.floor(r)).padStart(2, "0")}` : `${m}:${r < 10 ? "0" : ""}${r.toFixed(1)}`;
}
