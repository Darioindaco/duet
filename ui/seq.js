"use strict";
// Duet: the sequencer (FL style). Patterns hold notes for channels; a channel
// is a song track with an instrument. The channel rack has a row of steps per
// channel (a row per pad for drum kits); the piano roll edits one channel's
// notes in the open pattern. Patterns go into the playlist as blocks.
// Everything here edits the shared song, so your partner sees it right away.
//
// Loaded before the main script; runs after boot() calls seqSetup().

const STEP = 0.25;                 // a step is a 16th note (beats)
const DRUM_ROWS = [36, 38, 39, 42, 46, 49];   // shown for a drum channel by default
const PAD_NAMES = { 35: "Kick 2", 36: "Kick", 37: "Rim", 38: "Snare", 39: "Clap", 40: "Snare 2", 41: "Floor tom", 42: "Closed hat",
  43: "Low tom", 44: "Pedal hat", 45: "Mid tom", 46: "Open hat", 47: "Tom", 48: "High tom", 49: "Crash", 50: "Top tom", 51: "Ride" };
const NOTE_NAMES = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
const noteName = (k) => NOTE_NAMES[k % 12] + (Math.floor(k / 12) - 1);
const PR_SNAPS = [["1/16", 0.25], ["1/8", 0.5], ["1/4", 1], ["1/32", 0.125], ["Off", 0]];

const seq = {
  pat: null, channel: null, allPads: {},
  ppb: 96, rowH: 16, snap: 0.25, lastLen: null,
  sel: new Set(), drag: null, prTop: null,
};

// ---------- what the song holds ----------
const channels = () => (song.view ? song.view.tracks.filter((t) => t.inst) : []);
const patterns = () => (song.view ? song.view.patterns || [] : []);
const curPattern = () => patterns().find((p) => p.id === seq.pat) || null;
const patLen = (p) => (p && p.len) || 4 * bpb();
const patNotes = (pid, tid) => (song.view.notes || []).filter((n) => n.pattern === pid && (tid == null || n.track === tid));
const isDrums = (t) => t && (t.seq ? t.seq === "drums" : t.inst?.kind === "drums" || /drum/i.test(t.inst?.vst?.name || ""));
const chanLabel = (t) => (t.inst?.kind === "vst" ? t.inst.vst?.name || "VST" : INST[t.inst?.kind]?.name || "Instrument");
function patColor(pid) {
  let h = 2166136261;
  for (const c of String(pid)) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  return `hsl(${Math.round(((h >>> 0) % 1000) * 137.508) % 360} 62% 62%)`; // golden-angle steps: neighbours differ
}

// ---------- setup ----------
function seqSetup() {
  seq.ppb = store.get("prZoom", 96);
  seq.snap = store.get("prSnap", 0.25);
  $("patSel").onchange = () => {
    const v = $("patSel").value;
    if (v === "+new") newPattern(); else { seq.pat = v; seqRender(); }
  };
  $("patLen").onchange = () => seq.pat && songEdit([["pattern", seq.pat, { len: +$("patLen").value * bpb() }]]);
  $("patPlay").onclick = async () => {
    const st = song.st;
    if (st && st.playing) { await api.daw_stop(); return; }
    if (!seq.pat) newPattern();
    await api.daw_play_pattern(seq.pat);
  };
  $("patPlace").onclick = placePattern;
  $("patMore").onclick = patMenu;
  $("addChannel").onchange = () => { const v = $("addChannel").value; $("addChannel").value = ""; if (v) addChannel(v); };
  const snap = $("prSnap");
  for (const [label, v] of PR_SNAPS) snap.append(new Option("Snap: " + label, v, false, v === seq.snap));
  snap.onchange = () => { seq.snap = +snap.value; store.set("prSnap", seq.snap); };
  $("prZoomIn").onclick = () => prZoom(1.3);
  $("prZoomOut").onclick = () => prZoom(1 / 1.3);
  const pr = $("prLanes");
  pr.addEventListener("scroll", () => { $("prKeysIn").style.transform = `translateY(${-pr.scrollTop}px)`; prDraw(); });
  pr.addEventListener("pointerdown", prDown);
  pr.addEventListener("pointermove", prHover);
  pr.addEventListener("contextmenu", (e) => { e.preventDefault(); prErase(e); });
  pr.addEventListener("wheel", (e) => { if (e.ctrlKey || e.metaKey) { e.preventDefault(); prZoom(e.deltaY < 0 ? 1.15 : 1 / 1.15); } }, { passive: false });
  $("prVel").addEventListener("pointerdown", velDown);
  new ResizeObserver(() => prDraw()).observe($("prWrap"));
  document.addEventListener("keydown", seqKeys);
}

function seqKeys(e) {
  if (ui.view !== "seq" || e.target.closest("input, select, textarea") || !$("vstm").hidden) return;
  if (e.code === "Space") { e.preventDefault(); $("patPlay").click(); }
  else if ((e.key === "Delete" || e.key === "Backspace") && seq.sel.size) {
    e.preventDefault();
    const ids = [...seq.sel]; seq.sel.clear();
    songEdit(ids.map((id) => ["note", id, { del: true }]));
  } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
    e.preventDefault();
    for (const n of patNotes(seq.pat, seq.channel)) seq.sel.add(n.id);
    prDraw();
  } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") { e.preventDefault(); e.shiftKey ? redo() : undo(); }
  else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "y") { e.preventDefault(); redo(); }
}

// ---------- patterns ----------
function newPattern(name) {
  const n = patterns().length + 1;
  const id = newId("p");
  seq.pat = id;
  songEdit([["pattern", id, { name: name || `Pattern ${n}`, len: bpb() }]]);
  return id;
}

function placePattern() {
  const p = curPattern();
  if (!p) return;
  const v = song.view;
  // on the lane of the pattern's first channel, at the playhead (on a bar)
  const used = patNotes(p.id).map((n) => n.track);
  const lane = v.tracks.find((t) => used.includes(t.id)) || v.tracks.find((t) => t.id === seq.channel) || v.tracks[0];
  if (!lane) { toast("Add a track or channel first."); return; }
  const at = Math.max(0, Math.floor(curPos() / bpb() + 1e-9) * bpb());
  songEdit([["clip", newId("c"), { track: lane.id, pattern: p.id, start: at, blen: patLen(p), boff: 0 }]]);
  toast(`"${p.name}" is in the song at bar ${Math.round(at / bpb()) + 1}.`);
}

function patMenu(e) {
  const p = curPattern();
  if (!p) return;
  const r = $("patMore").getBoundingClientRect();
  showCtx([
    ["Rename…", "", () => { const n = prompt("Pattern name", p.name); if (n && n.trim()) songEdit([["pattern", p.id, { name: n.trim() }]]); }],
    ["Make a copy", "", () => {
      const id = newId("p");
      const ch = [["pattern", id, { name: p.name + " copy", len: patLen(p) }]];
      for (const n of patNotes(p.id)) ch.push(["note", newId("n"), { pattern: id, track: n.track, start: n.start, len: n.len, key: n.key, vel: n.vel }]);
      seq.pat = id;
      songEdit(ch);
    }],
    ["Clear its notes", "", () => songEdit(patNotes(p.id).map((n) => ["note", n.id, { del: true }]))],
    ["Delete the pattern", "", () => {
      const clips = song.view.clips.filter((c) => c.pattern === p.id);
      if (!confirm(`Delete "${p.name}"${clips.length ? ` and its ${clips.length} block${clips.length === 1 ? "" : "s"} in the playlist` : ""}?`)) return;
      seq.pat = null;
      songEdit([["pattern", p.id, { del: true }], ...clips.map((c) => ["clip", c.id, { del: true }]),
        ...patNotes(p.id).map((n) => ["note", n.id, { del: true }])]);
    }],
  ], r.left, r.bottom + 4);
}

// ---------- channels ----------
function addChannel(kind) {
  if (kind === "vst") { ui.instFor = "new-channel"; openVstBrowser("inst"); return; }
  makeChannel({ kind, ...(INST_DEFAULTS[kind] || {}) }, INST[kind].name);
}

// a channel is a song track with an instrument
function makeChannel(inst, name) {
  const v = song.view;
  const order = v.tracks.reduce((a, t) => Math.max(a, t.order || 0), 0) + 1;
  const id = newId("t");
  seq.channel = id;
  songEdit([["track", id, { name, order, owner: v.me, src: 0, arm: false, vol: 0.8, pan: 0, rack: [], inst }]]);
}

function selectChannel(tid) {
  seq.channel = tid;
  seq.sel.clear();
  seq.prTop = null; // scroll the piano roll to this channel's notes
  seqRender();
  syncMidiTarget();
}

// ---------- rendering ----------
function seqRender() {
  if (!song.view) return;
  const pats = patterns();
  if (!pats.some((p) => p.id === seq.pat)) seq.pat = pats[0]?.id || null;
  const chans = channels();
  if (!chans.some((t) => t.id === seq.channel)) seq.channel = chans[0]?.id || null;
  // pattern picker
  const sel = $("patSel");
  sel.innerHTML = "";
  for (const p of pats) sel.append(new Option(p.name || "Pattern", p.id, false, p.id === seq.pat));
  if (!pats.length) sel.append(new Option("No patterns yet", "", true, true));
  sel.append(new Option("+ New pattern", "+new"));
  const p = curPattern();
  $("patLen").value = String(Math.max(1, Math.round(patLen(p) / bpb())));
  $("patLen").disabled = $("patPlace").disabled = $("patMore").disabled = !p;
  $("patPlay").classList.toggle("on", !!(song.st && song.st.playing && song.view.patternMode));
  if (ui.view === "seq") { renderRackRows(); prRender(); }
}

function renderRackRows() {
  const box = $("chanRows"), p = curPattern(), chans = channels();
  box.innerHTML = "";
  if (!chans.length) {
    box.innerHTML = `<div class="seq-empty"><b>No channels yet.</b> A channel is an instrument the song plays: add a drum kit, the synth or a VST instrument with <b>+ Channel</b>. It also becomes a track in the playlist and a strip in the mixer.</div>`;
    return;
  }
  const steps = Math.max(1, Math.round(patLen(p) / STEP));
  for (const t of chans) {
    const drums = isDrums(t);
    const notes = p ? patNotes(p.id, t.id) : [];
    let rows = drums ? DRUM_ROWS.slice() : [60];
    if (drums) {
      for (const n of notes) if (!rows.includes(n.key)) rows.push(n.key);
      if (seq.allPads[t.id]) for (let k = 36; k <= 51; k++) if (!rows.includes(k)) rows.push(k);
      rows.sort((a, b) => a - b);
    }
    const ch = document.createElement("div");
    ch.className = "chan" + (t.id === seq.channel ? " sel" : "") + (t.mine ? "" : " partner");
    ch.dataset.id = t.id;
    const head = document.createElement("div");
    head.className = "chead";
    head.innerHTML = `<div class="cname"><b>${esc(t.name)}</b><span>${esc(chanLabel(t))}</span></div>`;
    head.onclick = () => selectChannel(t.id);
    const tools = document.createElement("div");
    tools.className = "ctools";
    tools.append(
      toggleBtn("M", "m", t.mute, "Mute (just for you)", (on) => api.daw_local("mute", on, t.id).then(setSongView)),
      toggleBtn("S", "s", t.solo, "Solo (just for you)", (on) => api.daw_local("solo", on, t.id).then(setSongView)),
      hslider(t.vol ?? 0.8, (g) => songEditThrottled(t.id, { vol: g }), "Volume"));
    const edit = document.createElement("button");
    edit.className = "fxb"; edit.textContent = "Sound"; edit.title = "The instrument's knobs, pads and effects (in the mixer)";
    edit.onclick = (e) => { e.stopPropagation(); openTrackFx(t); };
    tools.append(edit);
    if (drums) {
      const more = document.createElement("button");
      more.className = "fxb"; more.textContent = seq.allPads[t.id] ? "Fewer" : "All pads";
      more.title = "Show every pad, or just the usual ones";
      more.onclick = (e) => { e.stopPropagation(); seq.allPads[t.id] = !seq.allPads[t.id]; renderRackRows(); };
      tools.append(more);
    }
    head.append(tools);
    ch.append(head);
    const grid = document.createElement("div");
    grid.className = "cgrid";
    // a melody (notes the steps can't show): a little picture of the piano roll instead
    if (!drums && notes.some((n) => n.key !== 60 || Math.abs(n.len - STEP * 2) > 1e-6 || Math.abs(n.start / STEP - Math.round(n.start / STEP)) > 1e-6)) {
      const mini = document.createElement("canvas");
      mini.className = "minipr";
      mini.title = "Notes in the piano roll: click to edit them";
      grid.append(mini);
      requestAnimationFrame(() => {
        const w = mini.clientWidth, h = mini.clientHeight;
        const ctx = sizeCanvas(mini, w, h);
        const lo = Math.min(...notes.map((n) => n.key)), hi = Math.max(...notes.map((n) => n.key));
        const span = Math.max(8, hi - lo + 1), plen = patLen(p);
        ctx.fillStyle = "#16141c"; ctx.fillRect(0, 0, w, h);
        ctx.fillStyle = t.mine ? song.colors.me : song.colors.partner;
        for (const n of notes) ctx.fillRect(n.start / plen * w, 3 + (hi - n.key) / span * (h - 6), Math.max(2, n.len / plen * w - 1), Math.max(2, (h - 6) / span));
      });
      ch.append(grid);
      ch.onclick = (e) => { if (!e.target.closest("button, .hs")) selectChannel(t.id); };
      box.append(ch);
      continue;
    }
    for (const key of rows) {
      const row = document.createElement("div");
      row.className = "srow";
      row.innerHTML = `<span class="rname" title="Click to hear it">${esc(drums ? PAD_NAMES[key] || noteName(key) : "Steps (" + noteName(key) + ")")}</span>`;
      row.firstChild.onpointerdown = (e) => { e.preventDefault(); audition(t, key); };
      const cells = document.createElement("div");
      cells.className = "cells";
      cells.style.gridTemplateColumns = `repeat(${steps}, minmax(14px, 1fr))`;
      for (let s = 0; s < steps; s++) {
        const at = s * STEP;
        const hit = notes.find((n) => n.key === key && Math.abs(n.start - at) < 1e-6);
        const c = document.createElement("button");
        c.className = "step" + (hit ? " on" : "") + (Math.floor(s / 4) % 2 ? " odd" : "") + (s % (4 * bpb()) === 0 ? " bar" : "");
        if (hit) c.style.setProperty("--v", (hit.vel / 127).toFixed(2));
        c.onclick = () => {
          if (!p) { const pid = newPattern(); seq.pat = pid; }
          const pid = seq.pat;
          if (hit) songEdit([["note", hit.id, { del: true }]]);
          else {
            songEdit([["note", newId("n"), { pattern: pid, track: t.id, start: at, len: drums ? STEP : STEP * 2, key, vel: 100 }]]);
            audition(t, key);
          }
        };
        cells.append(c);
      }
      row.append(cells);
      grid.append(row);
    }
    ch.append(grid);
    ch.onclick = (e) => { if (!e.target.closest("button, .hs")) selectChannel(t.id); };
    box.append(ch);
  }
}

function audition(t, key, vel = 100) {
  if (t.slot == null) return;
  api.note("T" + t.slot, true, key, vel);
  setTimeout(() => api.note("T" + t.slot, false, key), 220);
}

// ---------- the piano roll ----------
function prChannel() { return channels().find((t) => t.id === seq.channel) || null; }

function prRange(t) {
  // drum kits: the pads (and a little either side); everything else: the whole keyboard
  return isDrums(t) ? [33, 54] : [21, 108];
}

function prRender() {
  const t = prChannel(), p = curPattern();
  $("prTitle").innerHTML = t ? `Piano roll <span class="who">· ${esc(t.name)}${p ? " · " + esc(p.name) : ""}</span>` : "Piano roll";
  const [lo, hi] = t ? prRange(t) : [48, 72];
  seq.rowH = t && isDrums(t) ? 22 : 16;
  const keys = $("prKeysIn");
  keys.innerHTML = "";
  keys.style.height = (hi - lo + 1) * seq.rowH + "px";
  for (let k = hi; k >= lo; k--) {
    const black = [1, 3, 6, 8, 10].includes(k % 12);
    const d = document.createElement("div");
    d.className = "pkey" + (black && !isDrums(t) ? " black" : "") + (k % 12 === 0 ? " c" : "");
    d.style.height = seq.rowH + "px";
    d.textContent = t && isDrums(t) ? PAD_NAMES[k] || "" : k % 12 === 0 ? noteName(k) : "";
    d.onpointerdown = (e) => { e.preventDefault(); if (t) audition(t, k); };
    keys.append(d);
  }
  $("prSize").style.height = (hi - lo + 1) * seq.rowH + "px";
  $("prSize").style.width = Math.max(patLen(p) + bpb(), 8) * seq.ppb + "px";
  const lanes = $("prLanes");
  if (seq.prTop == null && t) {
    // open where the notes are (or around middle C / the kick), once the layout has settled
    const ns = p ? patNotes(p.id, t.id) : [];
    const mid = ns.length ? ns.reduce((a, n) => a + n.key, 0) / ns.length : isDrums(t) ? 42 : 60;
    seq.prTop = 0;
    requestAnimationFrame(() => {
      seq.prTop = Math.max(0, (hi - mid + 0.5) * seq.rowH - lanes.clientHeight / 2);
      lanes.scrollTop = seq.prTop;
      prDraw();
    });
  }
  prDraw();
}

function prKeyAt(y) {
  const t = prChannel();
  const [, hi] = t ? prRange(t) : [48, 72];
  return hi - Math.floor(y / seq.rowH);
}

function prDraw() {
  if (ui.view !== "seq") return;
  const lanes = $("prLanes"), w = lanes.clientWidth, h = lanes.clientHeight;
  if (!w || !h) return;
  const ctx = sizeCanvas($("prCv"), w, h);
  const t = prChannel(), p = curPattern();
  const [lo, hi] = t ? prRange(t) : [48, 72];
  const sx = lanes.scrollLeft, sy = lanes.scrollTop, ppb = seq.ppb, rh = seq.rowH;
  ctx.fillStyle = "#100f14";
  ctx.fillRect(0, 0, w, h);
  for (let k = hi; k >= lo; k--) {
    const y = (hi - k) * rh - sy;
    if (y > h || y + rh < 0) continue;
    const black = [1, 3, 6, 8, 10].includes(k % 12) && !(t && isDrums(t));
    ctx.fillStyle = black ? "#0c0b0f" : (t && isDrums(t) && PAD_NAMES[k]) ? "#16141c" : "#131118";
    ctx.fillRect(0, y, w, rh);
    ctx.fillStyle = k % 12 === 0 ? "#2a2634" : "#1a1820";
    ctx.fillRect(0, y + rh - 1, w, 1);
  }
  const plen = patLen(p);
  const n = bpb();
  for (let b = Math.floor(sx / ppb / STEP) * STEP; b * ppb - sx < w; b += STEP) {
    const x = Math.round(b * ppb - sx) + 0.5;
    const bar = Math.abs(b % n) < 1e-6, beat = Math.abs(b % 1) < 1e-6;
    if (!beat && ppb < 40) continue;
    ctx.fillStyle = bar ? "rgba(255,255,255,0.13)" : beat ? "rgba(255,255,255,0.06)" : "rgba(255,255,255,0.025)";
    ctx.fillRect(x - 0.5, 0, 1, h);
  }
  // past the pattern's end: dimmed
  const xe = plen * ppb - sx;
  if (xe < w) { ctx.fillStyle = "rgba(0,0,0,0.45)"; ctx.fillRect(Math.max(0, xe), 0, w, h); }
  if (!t || !p) {
    ctx.fillStyle = "#5f586b";
    ctx.font = "600 13px system-ui, sans-serif";
    ctx.fillText(!t ? "Add a channel to write notes for it." : "Make a pattern (the menu at the top) to write notes in.", 16, 28);
    $("prVel").getContext("2d").clearRect(0, 0, 9999, 9999);
    return;
  }
  const col = t.mine ? song.colors.me : song.colors.partner;
  const d = seq.drag;
  const notes = patNotes(p.id, t.id).map((nn) => (d && d.moved && d.res[nn.id] ? { ...nn, ...d.res[nn.id] } : nn));
  if (d && d.ghost) notes.push(d.ghost);
  for (const nn of notes) {
    const x = nn.start * ppb - sx, y = (hi - nn.key) * rh - sy, nw = Math.max(3, nn.len * ppb - 1);
    if (x > w || x + nw < 0 || y > h || y + rh < 0) continue;
    const sel = seq.sel.has(nn.id);
    ctx.globalAlpha = 0.45 + 0.55 * ((nn.vel ?? 100) / 127);
    ctx.fillStyle = col;
    ctx.beginPath(); ctx.roundRect(x + 0.5, y + 1.5, nw, rh - 3, 3); ctx.fill();
    ctx.globalAlpha = 1;
    ctx.strokeStyle = sel ? "#fff" : "rgba(0,0,0,0.5)";
    ctx.lineWidth = sel ? 1.5 : 1;
    ctx.stroke();
    if (nw > 26 && rh >= 16) {
      ctx.fillStyle = "#15121a";
      ctx.font = "700 10px system-ui, sans-serif";
      ctx.fillText(isDrums(t) ? "" : noteName(nn.key), x + 4, y + rh - 5);
    }
  }
  // playhead while the pattern plays
  const st = song.st;
  if (st && st.playing && song.view.patternMode === p.id) {
    const px = (curPos() % plen) * ppb - sx;
    ctx.fillStyle = "#efeaf4";
    ctx.fillRect(Math.round(px), 0, 1, h);
  }
  velDraw(notes, t, sx);
}

function velDraw(notes, t, sx) {
  const cv = $("prVel"), w = $("prLanes").clientWidth, h = 54;
  const ctx = sizeCanvas(cv, w, h);
  ctx.fillStyle = "#0c0b10";
  ctx.fillRect(0, 0, w, h);
  const col = t.mine ? song.colors.me : song.colors.partner;
  for (const nn of notes) {
    const x = nn.start * seq.ppb - sx;
    if (x < -4 || x > w) continue;
    const vh = Math.max(2, (nn.vel / 127) * (h - 8));
    ctx.fillStyle = seq.sel.has(nn.id) ? "#fff" : col;
    ctx.fillRect(Math.round(x) + 1, h - vh, 3, vh);
    ctx.beginPath(); ctx.arc(Math.round(x) + 2.5, h - vh, 3, 0, 2 * Math.PI); ctx.fill();
  }
}

function prZoom(f) {
  const lanes = $("prLanes"), beatAt = (lanes.scrollLeft + lanes.clientWidth / 2) / seq.ppb;
  seq.ppb = Math.max(20, Math.min(480, seq.ppb * f));
  store.set("prZoom", seq.ppb);
  prRender();
  lanes.scrollLeft = beatAt * seq.ppb - lanes.clientWidth / 2;
}

function prPoint(e) {
  const lanes = $("prLanes"), r = lanes.getBoundingClientRect();
  const x = e.clientX - r.left, y = e.clientY - r.top;
  return { inside: x >= 0 && y >= 0 && x <= lanes.clientWidth && y <= lanes.clientHeight,
    beat: (x + lanes.scrollLeft) / seq.ppb, key: prKeyAt(y + lanes.scrollTop) };
}

function prHit(pt) {
  const t = prChannel(), p = curPattern();
  if (!t || !p) return null;
  for (const n of patNotes(p.id, t.id)) {
    if (n.key === pt.key && pt.beat >= n.start && pt.beat <= n.start + n.len) {
      const edge = (n.start + n.len - pt.beat) * seq.ppb <= 6 && n.len * seq.ppb > 12;
      return { n, zone: edge ? "end" : "body" };
    }
  }
  return null;
}

const prSnap = (b, free) => (free || !seq.snap ? b : Math.round(b / seq.snap) * seq.snap);

function prHover(e) {
  if (seq.drag || e.buttons) return;
  const h = prPoint(e).inside ? prHit(prPoint(e)) : null;
  $("prLanes").style.cursor = h ? (h.zone === "end" ? "e-resize" : "grab") : "crosshair";
}

function prDown(e) {
  if (e.button !== 0) return;
  const t = prChannel();
  let p = curPattern();
  const pt = prPoint(e);
  if (!pt.inside || !t) return;
  if (!p) { newPattern(); return; }
  const lanes = $("prLanes");
  lanes.setPointerCapture(e.pointerId);
  const h = prHit(pt);
  const free = e.altKey;
  const drums = isDrums(t);
  let d;
  if (!h) {
    // draw a new note, and drag to stretch it
    const start = Math.max(0, Math.floor(pt.beat / (seq.snap || STEP)) * (seq.snap || STEP));
    const len = seq.lastLen || (drums ? STEP : 1);
    d = { mode: "new", ghost: { id: "ghost", pattern: p.id, track: t.id, start, len, key: pt.key, vel: 100 }, moved: true, res: {} };
    audition(t, pt.key);
    if (!e.shiftKey) seq.sel.clear();
  } else {
    if (e.shiftKey) { seq.sel.has(h.n.id) ? seq.sel.delete(h.n.id) : seq.sel.add(h.n.id); prDraw(); return; }
    if (!seq.sel.has(h.n.id)) { seq.sel.clear(); seq.sel.add(h.n.id); }
    const sel = patNotes(p.id, t.id).filter((n) => seq.sel.has(n.id));
    d = { mode: h.zone === "end" ? "len" : "move", anchor: h.n, grab: pt.beat - h.n.start, key0: pt.key, sel, moved: false, res: {}, x0: e.clientX, y0: e.clientY };
    if (d.mode === "move") audition(t, h.n.key, h.n.vel);
  }
  seq.drag = d;
  const move = (ev) => {
    const q = prPoint(ev);
    if (d.mode === "new") {
      d.ghost.len = Math.max(seq.snap || STEP, prSnap(q.beat, ev.altKey) - d.ghost.start) || d.ghost.len;
      if (q.beat <= d.ghost.start + (seq.snap || STEP)) d.ghost.len = seq.lastLen || (drums ? STEP : 1);
    } else {
      if (!d.moved && Math.hypot(ev.clientX - d.x0, ev.clientY - d.y0) < 3) return;
      d.moved = true;
      if (d.mode === "len") {
        const len = Math.max(seq.snap || 0.03125, prSnap(q.beat, ev.altKey || free) - d.anchor.start);
        for (const n of d.sel) d.res[n.id] = { len: Math.max(0.03125, n.len + (len - d.anchor.len)) };
      } else {
        const db = Math.max(-Math.min(...d.sel.map((n) => n.start)), prSnap(q.beat - d.grab, ev.altKey || free) - d.anchor.start);
        const dk = q.key - d.key0;
        for (const n of d.sel) d.res[n.id] = { start: n.start + db, key: Math.max(0, Math.min(127, n.key + dk)) };
      }
    }
    prDraw();
  };
  const up = () => {
    lanes.removeEventListener("pointermove", move);
    lanes.removeEventListener("pointerup", up);
    seq.drag = null;
    if (d.mode === "new") {
      const id = newId("n");
      seq.lastLen = d.ghost.len;
      seq.sel.clear(); seq.sel.add(id);
      songEdit([["note", id, { pattern: d.ghost.pattern, track: d.ghost.track, start: d.ghost.start, len: d.ghost.len, key: d.ghost.key, vel: d.ghost.vel }]]);
    } else if (d.moved) {
      if (d.mode === "len") seq.lastLen = d.res[d.anchor.id]?.len || seq.lastLen;
      songEdit(Object.entries(d.res).map(([id, f]) => ["note", id, f]));
    }
    prDraw();
  };
  lanes.addEventListener("pointermove", move);
  lanes.addEventListener("pointerup", up);
  prDraw();
}

function prErase(e) {
  const h = prHit(prPoint(e));
  if (h) { seq.sel.delete(h.n.id); songEdit([["note", h.n.id, { del: true }]]); }
}

// velocity lane: drag up/down over the bars (selected notes move together)
function velDown(e) {
  const t = prChannel(), p = curPattern();
  if (!t || !p) return;
  const cv = $("prVel"), r = cv.getBoundingClientRect();
  cv.setPointerCapture(e.pointerId);
  const sx = $("prLanes").scrollLeft;
  const vals = {};
  const set = (ev) => {
    const beat = (ev.clientX - r.left + sx) / seq.ppb;
    const vel = Math.max(1, Math.min(127, Math.round((1 - (ev.clientY - r.top - 4) / (r.height - 8)) * 127)));
    const near = patNotes(p.id, t.id).filter((n) => Math.abs(n.start - beat) * seq.ppb < 6);
    const which = near.some((n) => seq.sel.has(n.id)) ? patNotes(p.id, t.id).filter((n) => seq.sel.has(n.id)) : near;
    for (const n of which) { n.vel = vel; vals[n.id] = vel; }
    prDraw();
  };
  const up = () => {
    cv.removeEventListener("pointermove", set);
    cv.removeEventListener("pointerup", up);
    songEdit(Object.entries(vals).map(([id, vel]) => ["note", id, { vel }]));
  };
  cv.addEventListener("pointermove", set);
  cv.addEventListener("pointerup", up);
  set(e);
}

// ---------- playing position in pattern mode ----------
function seqFrame() {
  if (ui.view !== "seq" || !song.st) return;
  const playing = song.st.playing && song.view?.patternMode;
  $("patPlay").classList.toggle("on", !!(song.st.playing));
  if (playing) {
    prDraw();
    const p = curPattern(), steps = Math.round(patLen(p) / STEP);
    const s = Math.floor((curPos() % patLen(p)) / STEP);
    for (const row of document.querySelectorAll("#chanRows .cells")) {
      const prev = row.querySelector(".step.now");
      if (prev) prev.classList.remove("now");
      if (s >= 0 && s < steps) row.children[s]?.classList.add("now");
    }
  }
}
