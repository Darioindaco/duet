"use strict";
// Duet: the song view. A playlist of tracks with recorded clips, a transport,
// and the song menu. The song itself lives in Python (daw.py), shared live with
// the partner; this page shows it and sends edits as [kind, id, {field: value}].
// Positions are in beats; clip offsets and lengths in seconds.
//
// Loaded before the main script; everything here runs once boot() calls
// songSetup(), so the main script's helpers ($, ui, api, toast…) exist by then.

const ROW = 64;          // track height, px
const SNAPS = [["Bar", 0], ["Beat", 1], ["1/2", 0.5], ["1/4", 0.25], ["Off", -1]];
const song = {
  view: null, v: -1, st: null, stAt: 0, fetching: false,
  peaks: {}, peakReq: {},
  sel: new Set(), selTrack: null,
  ppb: 22, snap: 1,          // pixels per beat, snap (beats; 0 = bar, -1 = off)
  drag: null, rulerDrag: null, headBusy: false, pendingHeads: false,
  cv: null, ov: null, ruler: null, raf: 0, follow: true,
  colors: { me: "#ff9548", partner: "#4fd6c6" },
};

// ---------- small helpers ----------
const spb = () => 60 / (song.view ? song.view.meta.bpm : 120);
const bpb = () => (song.view ? song.view.meta.beats : 4);
const trackRow = (tid) => song.view.tracks.findIndex((t) => t.id === tid);
const tColor = (t) => (t && !t.mine ? song.colors.partner : song.colors.me);
const clipBeats = (c) => c.len / spb();
function snapBeat(b, mode = song.snap) {
  if (mode < 0) return b;
  const g = mode === 0 ? bpb() : mode;
  return Math.round(b / g) * g;
}
function fmtPos(beats) {
  const n = bpb(), neg = beats < 0, b = Math.abs(beats);
  const bar = Math.floor(b / n), beat = Math.floor(b % n), tick = Math.floor((b % 1) * 100);
  return `${neg ? "-" : ""}${bar + 1}.${beat + 1}.${String(tick).padStart(2, "0")}`;
}
function fmtTime(beats) {
  const s = Math.max(0, beats * spb());
  return `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, "0")}`;
}
function curPos() {
  const st = song.st;
  if (!st) return 0;
  if (!st.playing) return st.pos;
  return st.pos + (performance.now() - song.stAt) / 1000 / spb();
}
function decodePeaks(b64) {
  const bin = atob(b64), out = new Int8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = (bin.charCodeAt(i) << 24) >> 24;
  return out;
}
function newId(prefix) { return prefix + Math.random().toString(36).slice(2, 12); }

// ---------- edits (applied here right away, then by Python, which answers with the song) ----------
function applyLocal(changes) {
  const v = song.view;
  for (const [kind, id, f] of changes) {
    if (kind === "meta") Object.assign(v.meta, f);
    const list = kind === "track" ? v.tracks : kind === "clip" ? v.clips : null;
    if (!list) continue;
    const o = list.find((x) => x.id === id);
    if (o) Object.assign(o, f);
    else list.push({ id, ...f, ...(kind === "track" ? { mine: f.owner === v.me, slot: null } : {}) });
    if (f.del) list.splice(list.findIndex((x) => x.id === id), 1);
  }
  if (changes.some((c) => c[0] === "track" && "order" in c[2])) v.tracks.sort((a, b) => a.order - b.order);
}
async function songEdit(changes) {
  if (!changes.length) return;
  applyLocal(changes);
  songRender();
  setSongView(await api.daw_edit(changes));
}
function setSongView(v) {
  if (!v) return;
  const before = song.view;
  song.view = v;
  for (const id of [...song.sel]) if (!v.clips.some((c) => c.id === id)) song.sel.delete(id);
  if (ui.sel[0] === "T" && !v.tracks.some((t) => "T" + t.slot === ui.sel)) { ui.sel = "L0"; renderRack(); }
  // track strips in the mixer follow the song's tracks (names, owners, slots)
  const sig = JSON.stringify(v.tracks.map((t) => [t.id, t.slot, t.name, t.mine]));
  if (!before || sig !== song.trackSig) {
    song.trackSig = sig;
    api.mixer().then((m) => { ui.mixer = m; renderStrips(); if (ui.sel[0] === "T") renderRack(); });
  }
  songRender();
  paintConflict();
}

// ---------- setup ----------
function songSetup() {
  const css = getComputedStyle(document.documentElement);
  song.colors.me = css.getPropertyValue("--me").trim() || song.colors.me;
  song.colors.partner = css.getPropertyValue("--partner").trim() || song.colors.partner;
  song.ppb = store.get("songZoom", 22);
  song.snap = store.get("songSnap", 1);
  song.cv = $("lanesCv"); song.ov = $("lanesOv"); song.ruler = $("ruler");

  // views
  for (const b of $("viewSeg").querySelectorAll("button")) b.onclick = () => setMainView(b.dataset.v);
  setMainView(store.get("view", "song"));

  // transport
  $("tpPlay").onclick = togglePlay;
  $("tpRec").onclick = toggleRec;
  $("songBpm").onchange = () => {
    const v = Math.max(40, Math.min(240, Math.round(+$("songBpm").value || 120)));
    songEdit([["meta", "meta", { bpm: v }]]);
  };
  $("songBeats").onchange = () => songEdit([["meta", "meta", { beats: +$("songBeats").value }]]);
  $("tgClick").onclick = async () => setSongView(await api.daw_local("click", !song.view.local.click));
  $("tgCount").onclick = async () => setSongView(await api.daw_local("countin", !song.view.local.countin));
  $("tgLoop").onclick = () => {
    const m = song.view.meta;
    const ch = { loop: !m.loop };
    if (!m.loop && m.loop_end <= m.loop_start) Object.assign(ch, { loop_start: 0, loop_end: 4 * bpb() });
    songEdit([["meta", "meta", ch]]);
  };
  const snapSel = $("snapSel");
  for (const [label, v] of SNAPS) snapSel.append(new Option("Snap: " + label, v, false, v === song.snap));
  snapSel.onchange = () => { song.snap = +snapSel.value; store.set("songSnap", song.snap); };
  $("zoomIn").onclick = () => zoomBy(1.4);
  $("zoomOut").onclick = () => zoomBy(1 / 1.4);
  $("addTrack").onclick = addTrack;

  // lanes: scrolling, clicks, drags
  const lanes = $("lanes");
  lanes.addEventListener("scroll", () => {
    $("theadsIn").style.transform = `translateY(${-lanes.scrollTop}px)`;
    if (song.autoScroll) song.autoScroll = false;
    else {
      // you're looking somewhere else: stop following the playhead for a moment
      song.follow = false; clearTimeout(song.followT);
      song.followT = setTimeout(() => (song.follow = true), 2500);
    }
    songDraw();
  });
  lanes.addEventListener("wheel", (e) => {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    zoomBy(e.deltaY < 0 ? 1.15 : 1 / 1.15, e.clientX - lanes.getBoundingClientRect().left);
  }, { passive: false });
  lanes.addEventListener("pointerdown", lanesDown);
  lanes.addEventListener("dblclick", lanesDbl);
  song.ruler.addEventListener("pointerdown", rulerDown);
  new ResizeObserver(() => songDraw()).observe($("lanewrap"));

  // song menu
  $("songBtn").onclick = toggleSongMenu;
  document.addEventListener("pointerdown", (e) => {
    if (!$("songMenu").hidden && !e.target.closest("#songMenu, #songBtn")) $("songMenu").hidden = true;
  });
  $("cfTheirs").onclick = async () => { $("conflict").hidden = true; setSongView(await api.daw_resolve("theirs")); };
  $("cfMine").onclick = async () => { $("conflict").hidden = true; setSongView(await api.daw_resolve("mine")); };

  document.addEventListener("keydown", songKeys);
  api.daw_project().then(setSongView);
  const tick = () => { songFrame(); song.raf = requestAnimationFrame(tick); };
  tick();
}

function setMainView(v) {
  ui.view = v === "mixer" ? "mixer" : "song";
  store.set("view", ui.view);
  $("viewSong").hidden = ui.view !== "song";
  $("viewMixer").hidden = ui.view !== "mixer";
  for (const b of $("viewSeg").querySelectorAll("button")) b.classList.toggle("on", b.dataset.v === ui.view);
  if (ui.view === "song") requestAnimationFrame(songDraw);
}

function songKeys(e) {
  if (ui.view !== "song" || e.target.closest("input, select, textarea") || !$("vstm").hidden) return;
  if (e.target.closest("button")) e.target.blur(); // or space would press that button too
  if (e.code === "Space") { e.preventDefault(); togglePlay(); }
  else if (e.key === "r" || e.key === "R") { e.preventDefault(); toggleRec(); }
  else if (e.key === "Home") { e.preventDefault(); api.daw_locate(0); }
  else if ((e.key === "Delete" || e.key === "Backspace") && song.sel.size) {
    e.preventDefault();
    const ids = [...song.sel];
    song.sel.clear();
    songEdit(ids.map((id) => ["clip", id, { del: true }]));
  } else if (e.key === "Escape") { song.sel.clear(); songDraw(); }
  else if ((e.ctrlKey || e.metaKey) && e.key === "a") {
    e.preventDefault();
    song.view.clips.forEach((c) => song.sel.add(c.id));
    songDraw();
  }
}

// ---------- transport ----------
async function togglePlay() {
  const st = song.st;
  if (st && (st.playing || st.recording)) await api.daw_stop();
  else await api.daw_play(null);
}
async function toggleRec() {
  const st = song.st;
  if (st && st.recording) { await api.daw_stop(); return; }
  const res = await api.daw_record(null);
  if (res && res.error) toast(res.error);
}
function zoomBy(f, anchorX) {
  const lanes = $("lanes");
  const ax = anchorX ?? lanes.clientWidth / 2;
  const beatAt = (lanes.scrollLeft + ax) / song.ppb;
  song.ppb = Math.max(2, Math.min(240, song.ppb * f));
  store.set("songZoom", song.ppb);
  songRender();
  lanes.scrollLeft = beatAt * song.ppb - ax;
}

// ---------- polling (from the main paint loop) ----------
function songPaint(d) {
  if (!d) return;
  const was = song.st;
  song.st = d; song.stAt = performance.now();
  for (const n of d.notices || []) toast(n);
  if (d.v !== song.v && !song.fetching) {
    song.fetching = true;
    const v = d.v;
    api.daw_project().then((p) => { song.v = v; setSongView(p); }).finally(() => (song.fetching = false));
  }
  $("tpPlay").classList.toggle("on", d.playing);
  $("tpPlay").innerHTML = d.playing
    ? `<svg width="16" height="16" viewBox="0 0 24 24" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor"/></svg>`
    : `<svg width="16" height="16" viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z" fill="currentColor"/></svg>`;
  $("tpRec").classList.toggle("on", d.recording);
  $("lcd").classList.toggle("rec", d.recording);
  // file transfers
  const names = Object.keys(d.xfer || {});
  $("xfer").innerHTML = names.slice(0, 3).map((n) => {
    const [done, tot, dir] = d.xfer[n];
    const p = tot ? Math.round(done / tot * 100) : 0;
    return `<span class="xchip ${dir}" title="${dir === "out" ? "Sending a take to your partner" : "Getting a take from your partner"}">${dir === "out" ? "↑" : "↓"} take <i style="--p:${p}%"></i>${tot ? p + "%" : "…"}</span>`;
  }).join("");
  if (was && was.xfer && Object.keys(was.xfer).length !== names.length) songDraw();
  // meters
  for (const t of song.view ? song.view.tracks : []) {
    const m = document.querySelector(`.th[data-id="${t.id}"] .lv i`);
    if (m) m.style.height = dbPct((d.levels || {})[t.id] || 0) + "%";
    const s = t.slot != null && ui.meters["T" + t.slot];
    if (s) s.style.height = dbPct((d.levels || {})[t.id] || 0) + "%";
  }
}

// ---------- rendering ----------
function songRender() {
  if (!song.view) return;
  const v = song.view, m = v.meta;
  $("songName").textContent = m.name;
  $("songShared").classList.toggle("on", v.shared);
  $("songBtn").title = v.shared ? "This song is open on both sides: you're working on it together" : "Only on this computer for now. Connect to work on it together.";
  if (document.activeElement !== $("songBpm")) $("songBpm").value = Math.round(m.bpm);
  $("songBeats").value = String(m.beats);
  $("tgClick").classList.toggle("on", !!v.local.click);
  $("tgCount").classList.toggle("on", !!v.local.countin);
  $("tgLoop").classList.toggle("on", !!m.loop);
  if (song.headBusy) song.pendingHeads = true;
  else renderHeads();
  songDraw();
}

function srcOptions(t) {
  const v = song.view, opts = [];
  for (const [i, s] of ui.mixer.local.entries()) if (s.active) opts.push([`${v.me}|${i}`, `You · ${stripName("L" + i)}`]);
  if (v.partner) for (const s of ui.partnerStrips || []) opts.push([`${v.partner}|${s.i}`, `Partner · ${s.name}`]);
  const cur = `${t.owner}|${t.src}`;
  if (!opts.some((o) => o[0] === cur)) opts.push([cur, t.mine ? `You · In ${t.src + 1}` : `Partner · input ${t.src + 1}`]);
  return { opts, cur };
}

function renderHeads() {
  song.pendingHeads = false;
  const box = $("theadsIn"), v = song.view;
  box.innerHTML = "";
  box.style.height = v.tracks.length * ROW + "px";
  v.tracks.forEach((t, i) => {
    const el = document.createElement("div");
    el.className = "th" + (t.mine ? "" : " partner") + (song.selTrack === t.id ? " sel" : "");
    el.dataset.id = t.id;
    el.style.top = i * ROW + "px";
    el.onpointerdown = () => { song.selTrack = t.id; for (const x of box.children) x.classList.toggle("sel", x === el); };

    const name = document.createElement("input");
    name.className = "tn"; name.value = t.name || "Track"; name.spellcheck = false; name.title = "Rename";
    name.onfocus = () => (song.headBusy = true);
    name.onblur = () => { song.headBusy = false; if (name.value.trim() && name.value !== t.name) songEdit([["track", t.id, { name: name.value.trim() }]]); else if (song.pendingHeads) renderHeads(); };
    name.onkeydown = (e) => { if (e.key === "Enter") name.blur(); if (e.key === "Escape") { name.value = t.name; name.blur(); } };

    const btns = document.createElement("div");
    btns.className = "tbtns";
    const arm = document.createElement("button");
    arm.className = "arm" + (t.arm ? " on" : "");
    arm.title = t.mine ? "Record on this track (from your input)" : "Record on this track (from your partner's input, on their side)";
    arm.onclick = () => songEdit([["track", t.id, { arm: !t.arm }]]);
    const del = document.createElement("button");
    del.className = "del"; del.textContent = "✕"; del.title = "Delete this track";
    del.onclick = () => deleteTrack(t);
    btns.append(arm,
      toggleBtn("M", "m", t.mute, "Mute (just for you)", (on) => api.daw_local("mute", on, t.id).then(setSongView)),
      toggleBtn("S", "s", t.solo, "Solo (just for you)", (on) => api.daw_local("solo", on, t.id).then(setSongView)), del);

    const row2 = document.createElement("div");
    row2.className = "row2";
    const src = document.createElement("select");
    src.className = "tsrc"; src.title = "Who plays on this track, and from which input";
    const { opts, cur } = srcOptions(t);
    for (const [val, label] of opts) src.append(new Option(label, val, false, val === cur));
    src.onfocus = () => (song.headBusy = true);
    src.onblur = () => { song.headBusy = false; if (song.pendingHeads) renderHeads(); };
    src.onchange = () => {
      const [owner, s] = src.value.split("|");
      songEdit([["track", t.id, { owner, src: +s }]]);
    };
    const vol = hslider(t.vol ?? 0.8, (g) => songEditThrottled(t.id, { vol: g }), `Volume: ${fmtDb(t.vol ?? 0.8)}`);
    const fx = document.createElement("button");
    fx.className = "fxb" + ((t.rack || []).length ? " has" : "");
    fx.textContent = (t.rack || []).length ? `FX ${t.rack.length}` : "FX";
    fx.title = "Effects on this track (both of you hear them)";
    fx.onclick = () => openTrackFx(t);
    row2.append(src, vol, fx);
    el.append(name, btns, row2);
    el.insertAdjacentHTML("beforeend", `<div class="lv"><i></i></div>`);
    box.append(el);
  });
  if (!v.tracks.length) box.innerHTML = `<div class="song-empty">No tracks yet. Add one with <b>+ Track</b>.</div>`;
}

function hslider(value, onChange, title) {
  const el = document.createElement("div");
  el.className = "hs"; el.tabIndex = 0; el.title = title;
  el.innerHTML = `<div class="fill"></div><div class="cap"></div>`;
  let pos = faderPos(value);
  const place = () => { el.querySelector(".fill").style.width = pos * 100 + "%"; el.querySelector(".cap").style.left = pos * 100 + "%"; };
  const set = (p) => { pos = Math.max(0, Math.min(1, p)); place(); const g = faderGain(pos); el.title = `Volume: ${fmtDb(g)}`; onChange(g); };
  el.addEventListener("pointerdown", (e) => {
    e.stopPropagation();
    song.headBusy = true;
    el.setPointerCapture(e.pointerId);
    const r = el.getBoundingClientRect();
    set((e.clientX - r.left) / r.width);
    const move = (ev) => set((ev.clientX - r.left) / r.width);
    const up = () => { el.removeEventListener("pointermove", move); el.removeEventListener("pointerup", up); song.headBusy = false; if (song.pendingHeads) renderHeads(); };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  });
  el.addEventListener("dblclick", () => set(faderPos(1)));
  el.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight") set(pos + 0.02);
    if (e.key === "ArrowLeft") set(pos - 0.02);
  });
  place();
  return el;
}

// fader drags send at most ~15 edits a second
const pendingVol = {};
function songEditThrottled(tid, fields) {
  pendingVol[tid] = { ...(pendingVol[tid] || {}), ...fields };
  if (songEditThrottled.t) return;
  songEditThrottled.t = setTimeout(() => {
    songEditThrottled.t = null;
    const ch = Object.entries(pendingVol).map(([id, f]) => ["track", id, f]);
    for (const k of Object.keys(pendingVol)) delete pendingVol[k];
    songEdit(ch);
  }, 66);
}

function addTrack() {
  const v = song.view;
  const order = v.tracks.reduce((a, t) => Math.max(a, t.order || 0), 0) + 1;
  const firstInput = ui.mixer.local.findIndex((s) => s.active);
  const n = v.tracks.length + 1;
  songEdit([["track", newId("t"), { name: `Track ${n}`, order, owner: v.me, src: Math.max(0, firstInput), arm: false, vol: 0.8, pan: 0, rack: [] }]]);
  requestAnimationFrame(() => ($("lanes").scrollTop = 1e6));
}

function deleteTrack(t) {
  const clips = song.view.clips.filter((c) => c.track === t.id);
  if (clips.length && !confirm(`Delete "${t.name}" and its ${clips.length} clip${clips.length === 1 ? "" : "s"}? This deletes it for your partner too.`)) return;
  songEdit([["track", t.id, { del: true }], ...clips.map((c) => ["clip", c.id, { del: true }])]);
}

function trackBySlot(slot) {
  return song.view ? song.view.tracks.find((t) => t.slot === slot) : null;
}

function openTrackFx(t) {
  if (t.slot == null) { toast("This song has more tracks than the mixer can hold (32)."); return; }
  setMainView("mixer");
  selectStrip("T" + t.slot);
}

// ---------- drawing ----------
function sizeCanvas(cv, w, h) {
  const dpr = window.devicePixelRatio || 1;
  if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) {
    cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr);
    cv.style.width = w + "px"; cv.style.height = h + "px";
  }
  const ctx = cv.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
}

function songLengthBeats() {
  const v = song.view;
  let end = 64 * bpb();
  for (const c of v.clips) end = Math.max(end, c.start + clipBeats(c));
  if (v.meta.loop) end = Math.max(end, v.meta.loop_end);
  return end + 16 * bpb();
}

function gridStep() {
  // the finest grid that stays readable
  const n = bpb();
  if (song.ppb >= 48) return 0.25;
  if (song.ppb >= 12) return 1;
  if (song.ppb * n >= 24) return n;
  return n * 4;
}

function songDraw() {
  if (!song.view || ui.view !== "song") return;
  const lanes = $("lanes");
  const w = lanes.clientWidth, h = lanes.clientHeight;
  if (!w || !h) return;
  const v = song.view, ppb = song.ppb, sx = lanes.scrollLeft, sy = lanes.scrollTop;
  $("lanesSize").style.width = songLengthBeats() * ppb + "px";
  $("lanesSize").style.height = Math.max(h, (v.tracks.length + 2) * ROW) + "px";
  const ctx = sizeCanvas(song.cv, w, h);
  ctx.clearRect(0, 0, w, h);
  // rows
  v.tracks.forEach((t, i) => {
    const y = i * ROW - sy;
    if (y > h || y + ROW < 0) return;
    ctx.fillStyle = i % 2 ? "#111017" : "#14121a";
    ctx.fillRect(0, y, w, ROW);
    ctx.fillStyle = "#1f1c27";
    ctx.fillRect(0, y + ROW - 1, w, 1);
  });
  // grid
  const n = bpb(), step = gridStep();
  const b0 = Math.floor(sx / ppb / step) * step, b1 = (sx + w) / ppb;
  const rowsH = Math.min(h, v.tracks.length * ROW - sy);
  for (let b = b0; b <= b1; b += step) {
    const x = Math.round(b * ppb - sx) + 0.5;
    const bar = Math.abs(b % n) < 1e-6;
    ctx.fillStyle = bar ? "rgba(255,255,255,0.10)" : Number.isInteger(b) ? "rgba(255,255,255,0.045)" : "rgba(255,255,255,0.02)";
    ctx.fillRect(x - 0.5, 0, 1, Math.max(0, rowsH));
  }
  // loop
  if (v.meta.loop) {
    const x0 = v.meta.loop_start * ppb - sx, x1 = v.meta.loop_end * ppb - sx;
    ctx.fillStyle = "rgba(79,214,198,0.05)";
    ctx.fillRect(x0, 0, x1 - x0, Math.max(0, rowsH));
  }
  // clips
  const d = song.drag;
  for (const c of v.clips) {
    let row = trackRow(c.track), start = c.start;
    if (row < 0) continue;
    if (d && d.moved && song.sel.has(c.id)) {
      start = Math.max(0, d.orig[c.id].start + d.db);
      row = Math.max(0, Math.min(v.tracks.length - 1, d.orig[c.id].row + d.dr));
    }
    const x = start * ppb - sx, cw = clipBeats(c) * ppb, y = row * ROW - sy;
    if (x > w || x + cw < 0 || y > h || y + ROW < 0) continue;
    drawClip(ctx, c, x, y + 2, cw, ROW - 5, v.tracks[row]);
  }
  drawRuler();
  drawOverlay();
}

function drawClip(ctx, c, x, y, w, h, track) {
  const col = tColor(track), sel = song.sel.has(c.id);
  const r = 6;
  ctx.save();
  ctx.beginPath();
  ctx.roundRect(x + 0.5, y + 0.5, Math.max(2, w - 1), h - 1, r);
  ctx.fillStyle = col + "26";
  ctx.fill();
  ctx.clip();
  ctx.fillStyle = col + (sel ? "d0" : "80");
  ctx.fillRect(x, y, w, 15);
  const file = song.view.files[c.file];
  const wy = y + 17, wh = h - 19, mid = wy + wh / 2;
  if (file && file.here) {
    const pk = song.peaks[c.file];
    if (pk) {
      const sr = file.sr || 48000, per = 512, secPerPx = spb() / song.ppb;
      const lv = document.querySelector("#lanes").clientWidth;
      const px0 = Math.max(0, Math.floor(-x)), px1 = Math.min(Math.ceil(w), Math.ceil(lv - x));
      ctx.fillStyle = col + "e0";
      const g = (c.gain ?? 1) * (wh / 2) / 127;
      for (let px = px0; px < px1; px++) {
        const t0 = c.offset + px * secPerPx;
        let i0 = Math.floor(t0 * sr / per), i1 = Math.max(i0 + 1, Math.floor((t0 + secPerPx) * sr / per));
        let lo = 0, hi = 0;
        for (let i = i0; i < i1 && 2 * i + 1 < pk.length; i++) {
          if (i < 0) continue;
          lo = Math.min(lo, pk[2 * i]); hi = Math.max(hi, pk[2 * i + 1]);
        }
        const top = mid - hi * g, bot = mid - lo * g;
        ctx.fillRect(x + px, top, 1, Math.max(1, bot - top));
      }
    } else if (!song.peakReq[c.file]) {
      song.peakReq[c.file] = true;
      api.daw_peaks(c.file).then((b) => { if (b) { song.peaks[c.file] = decodePeaks(b); songDraw(); } else song.peakReq[c.file] = false; });
    }
  } else {
    // audio still on its way from the partner
    ctx.strokeStyle = col + "40";
    ctx.lineWidth = 6;
    for (let k = -wh; k < w; k += 16) { ctx.beginPath(); ctx.moveTo(x + k, wy + wh); ctx.lineTo(x + k + wh, wy); ctx.stroke(); }
    const xf = (song.st && song.st.xfer || {})[c.file];
    const label = xf && xf[1] ? `Receiving ${Math.round(xf[0] / xf[1] * 100)}%` : song.view.shared ? "Waiting for the audio…" : "Audio is with your partner";
    ctx.fillStyle = "#cfc8da";
    ctx.font = "600 11px system-ui, sans-serif";
    ctx.fillText(label, Math.max(x, 0) + 8, mid + 4);
  }
  ctx.fillStyle = "#121016";
  ctx.font = "700 10.5px system-ui, sans-serif";
  const label = track ? track.name : "Clip";
  ctx.fillText(label, Math.max(x, 0) + 6, y + 11);
  ctx.restore();
  ctx.beginPath();
  ctx.roundRect(x + 0.5, y + 0.5, Math.max(2, w - 1), h - 1, r);
  ctx.strokeStyle = sel ? "#ffffff" : col + "c0";
  ctx.lineWidth = sel ? 1.5 : 1;
  ctx.stroke();
}

function drawRuler() {
  const cv = song.ruler, w = $("lanewrap").clientWidth, h = 26;
  const ctx = sizeCanvas(cv, w, h);
  const v = song.view, ppb = song.ppb, sx = $("lanes").scrollLeft, n = bpb();
  ctx.fillStyle = "#16141c";
  ctx.fillRect(0, 0, w, h);
  if (v.meta.loop || song.rulerDrag) {
    const x0 = v.meta.loop_start * ppb - sx, x1 = v.meta.loop_end * ppb - sx;
    ctx.fillStyle = v.meta.loop ? "rgba(79,214,198,0.55)" : "rgba(255,255,255,0.14)";
    ctx.fillRect(x0, 0, x1 - x0, 8);
  }
  const barPx = ppb * n;
  const every = barPx >= 36 ? 1 : barPx >= 14 ? 4 : 8;
  ctx.font = "600 10.5px system-ui, sans-serif";
  for (let bar = Math.max(0, Math.floor(sx / barPx)); bar * barPx - sx < w; bar++) {
    const x = Math.round(bar * barPx - sx) + 0.5;
    if (bar % every === 0) {
      ctx.fillStyle = "#5f586b"; ctx.fillRect(x - 0.5, 10, 1, 16);
      ctx.fillStyle = "#8e879b"; ctx.fillText(String(bar + 1), x + 4, 21);
    } else if (every === 1 || bar % (every / 2) === 0) {
      ctx.fillStyle = "#2f2b38"; ctx.fillRect(x - 0.5, 18, 1, 8);
    }
    if (barPx >= 60) for (let k = 1; k < n; k++) { ctx.fillStyle = "#2a2634"; ctx.fillRect(Math.round(x + k * ppb), 21, 1, 5); }
  }
  // playhead marker
  const px = curPos() * ppb - sx;
  ctx.fillStyle = song.st && song.st.recording ? "#ff6070" : "#efeaf4";
  ctx.beginPath(); ctx.moveTo(px - 5, 12); ctx.lineTo(px + 5, 12); ctx.lineTo(px, 20); ctx.fill();
}

function drawOverlay() {
  const lanes = $("lanes"), w = lanes.clientWidth, h = lanes.clientHeight;
  const ctx = sizeCanvas(song.ov, w, h);
  ctx.clearRect(0, 0, w, h);
  if (!song.view) return;
  const v = song.view, ppb = song.ppb, sx = lanes.scrollLeft, sy = lanes.scrollTop;
  const pos = curPos(), st = song.st;
  // a take being recorded grows on its armed tracks
  if (st && st.recording && st.recFrom != null) {
    const x0 = st.recFrom * ppb - sx, x1 = pos * ppb - sx;
    v.tracks.forEach((t, i) => {
      if (!t.arm || x1 <= x0) return;
      const y = i * ROW - sy + 2;
      ctx.fillStyle = "rgba(255,96,112,0.16)";
      ctx.fillRect(x0, y, x1 - x0, ROW - 5);
      ctx.fillStyle = "rgba(255,96,112,0.85)";
      ctx.fillRect(x0, y, x1 - x0, 2);
      ctx.font = "700 10.5px system-ui, sans-serif";
      ctx.fillText(t.mine ? "Recording…" : "Partner recording…", x0 + 6, y + ROW - 12);
    });
  }
  const px = Math.round(pos * ppb - sx) + 0.5;
  ctx.fillStyle = st && st.recording ? "#ff6070" : "#efeaf4";
  ctx.fillRect(px - 0.5, 0, 1, Math.min(h, v.tracks.length * ROW - sy));
}

let lastFrameDraw = 0;
function songFrame() {
  if (!song.view || ui.view !== "song") return;
  const st = song.st;
  const pos = curPos();
  $("tpPos").textContent = fmtPos(pos);
  $("tpTime").textContent = fmtTime(pos);
  if (st && st.playing) {
    const lanes = $("lanes"), x = pos * song.ppb - lanes.scrollLeft;
    if (song.follow && (x > lanes.clientWidth - 30 || x < 0)) {
      const to = Math.max(0, pos * song.ppb - 40);
      if (Math.abs(to - lanes.scrollLeft) >= 1) { song.autoScroll = true; lanes.scrollLeft = to; }
    }
    drawRuler(); drawOverlay();
  } else if (performance.now() - lastFrameDraw > 250) {
    lastFrameDraw = performance.now();
    drawRuler(); drawOverlay();
  }
}

// ---------- mouse: lanes ----------
function hitClip(beat, row) {
  const v = song.view, t = v.tracks[row];
  if (!t) return null;
  for (let i = v.clips.length - 1; i >= 0; i--) {
    const c = v.clips[i];
    if (c.track === t.id && beat >= c.start && beat <= c.start + clipBeats(c)) return c;
  }
  return null;
}

function lanesDown(e) {
  if (e.button !== 0 || !song.view) return;
  const lanes = $("lanes"), r = lanes.getBoundingClientRect();
  const x = e.clientX - r.left, y = e.clientY - r.top;
  if (x > lanes.clientWidth || y > lanes.clientHeight) return; // the scrollbars
  const beat = (x + lanes.scrollLeft) / song.ppb, row = Math.floor((y + lanes.scrollTop) / ROW);
  const c = hitClip(beat, row);
  if (!c) {
    if (!e.shiftKey) song.sel.clear();
    const at = Math.max(0, snapBeat(beat));
    api.daw_locate(at);
    if (song.st && !song.st.playing) { song.st.pos = at; }
    songDraw();
    return;
  }
  if (e.shiftKey) { song.sel.has(c.id) ? song.sel.delete(c.id) : song.sel.add(c.id); songDraw(); return; }
  if (!song.sel.has(c.id)) { song.sel.clear(); song.sel.add(c.id); }
  const orig = {};
  for (const id of song.sel) {
    const cc = song.view.clips.find((k) => k.id === id);
    if (cc) orig[id] = { start: cc.start, row: trackRow(cc.track) };
  }
  song.drag = { x0: e.clientX, y0: e.clientY, grab: beat - c.start, anchor: c, orig, db: 0, dr: 0, moved: false };
  lanes.setPointerCapture(e.pointerId);
  lanes.classList.add("grab");
  const move = (ev) => {
    const d = song.drag;
    if (!d.moved && Math.hypot(ev.clientX - d.x0, ev.clientY - d.y0) < 4) return;
    d.moved = true;
    const b = (ev.clientX - r.left + lanes.scrollLeft) / song.ppb;
    const newStart = snapBeat(b - d.grab, ev.altKey ? -1 : song.snap);
    d.db = newStart - d.orig[d.anchor.id].start;
    d.dr = Math.floor((ev.clientY - r.top + lanes.scrollTop) / ROW) - d.orig[d.anchor.id].row;
    songDraw();
  };
  const up = () => {
    lanes.removeEventListener("pointermove", move);
    lanes.removeEventListener("pointerup", up);
    lanes.classList.remove("grab");
    const d = song.drag;
    song.drag = null;
    if (!d.moved) { songDraw(); return; }
    const v = song.view, ch = [];
    for (const [id, o] of Object.entries(d.orig)) {
      const row = Math.max(0, Math.min(v.tracks.length - 1, o.row + d.dr));
      ch.push(["clip", id, { start: Math.max(0, o.start + d.db), track: v.tracks[row].id }]);
    }
    songEdit(ch);
  };
  lanes.addEventListener("pointermove", move);
  lanes.addEventListener("pointerup", up);
  songDraw();
}

function lanesDbl(e) {
  // double-click a clip: select everything on its track from there on
  const lanes = $("lanes"), r = lanes.getBoundingClientRect();
  const beat = (e.clientX - r.left + lanes.scrollLeft) / song.ppb, row = Math.floor((e.clientY - r.top + lanes.scrollTop) / ROW);
  const c = hitClip(beat, row);
  if (!c) return;
  for (const k of song.view.clips) if (k.track === c.track && k.start >= c.start) song.sel.add(k.id);
  songDraw();
}

// ---------- mouse: ruler (click = play from here, drag = mark the loop) ----------
function rulerDown(e) {
  if (e.button !== 0 || !song.view) return;
  const cv = song.ruler, r = cv.getBoundingClientRect(), sx = $("lanes").scrollLeft;
  const at = (ev) => Math.max(0, snapBeat((ev.clientX - r.left + sx) / song.ppb, song.snap < 0 ? 1 : song.snap));
  const b0 = at(e);
  song.rulerDrag = { b0, moved: false };
  cv.setPointerCapture(e.pointerId);
  const m = song.view.meta, before = { loop: m.loop, loop_start: m.loop_start, loop_end: m.loop_end };
  let lastEv = e;
  const move = (ev) => {
    lastEv = ev;
    const b = at(ev);
    if (b === b0 && !song.rulerDrag.moved) return;
    song.rulerDrag.moved = true;
    Object.assign(m, { loop_start: Math.min(b0, b), loop_end: Math.max(b0, b) });
    drawRuler(); songDraw();
  };
  const up = () => {
    cv.removeEventListener("pointermove", move);
    cv.removeEventListener("pointerup", up);
    const moved = song.rulerDrag.moved;
    song.rulerDrag = null;
    if (moved && m.loop_end > m.loop_start) {
      Object.assign(m, before);
      songEdit([["meta", "meta", { loop: true, loop_start: Math.min(b0, at(lastEv)), loop_end: Math.max(b0, at(lastEv)) }]]);
    } else {
      Object.assign(m, before);
      api.daw_locate(b0);
      if (song.st && !song.st.playing) song.st.pos = b0;
      songDraw();
    }
  };
  cv.addEventListener("pointermove", move);
  cv.addEventListener("pointerup", up);
}

// ---------- song menu ----------
async function toggleSongMenu() {
  const menu = $("songMenu");
  if (!menu.hidden) { menu.hidden = true; return; }
  const r = $("songBtn").getBoundingClientRect();
  menu.style.left = r.left + "px";
  menu.style.top = r.bottom + 8 + "px";
  const v = song.view;
  menu.innerHTML = `<label class="lbl-sm" for="songRename">Song name</label>
    <input type="text" id="songRename" spellcheck="false" autocomplete="off">
    <div class="who${v.shared ? " on" : ""}"><i></i>${v.shared ? "Open on both sides: you're working on it together" : "Only on this computer. Connect, and your partner opens it too."}</div>
    <button class="btn block" id="songNew">+ New song</button>
    <div class="lbl-sm" style="margin-top:12px">Your songs</div>
    <div class="songs" id="songList"><div class="lbl-sm">Loading…</div></div>`;
  menu.hidden = false;
  const inp = $("songRename");
  inp.value = v.meta.name;
  inp.onchange = () => inp.value.trim() && songEdit([["meta", "meta", { name: inp.value.trim() }]]);
  inp.onkeydown = (e) => e.key === "Enter" && inp.blur();
  $("songNew").onclick = async () => {
    if (v.shared && !confirm("Start a new song? Your partner switches to it too (this one stays saved).")) return;
    menu.hidden = true;
    setSongView(await api.daw_new("New song"));
  };
  const list = await api.daw_projects();
  $("songList").innerHTML = "";
  for (const p of list) {
    const b = document.createElement("button");
    b.className = "song" + (p.current ? " cur" : "");
    const when = new Date(p.modified * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
    b.innerHTML = `<b>${esc(p.name)}</b><span>${p.clips} clip${p.clips === 1 ? "" : "s"} · ${when}</span>`;
    if (!p.current) b.onclick = async () => {
      if (v.shared && !confirm(`Open "${p.name}"? Your partner switches to it too.`)) return;
      menu.hidden = true;
      setSongView(await api.daw_open(p.pid));
    };
    $("songList").append(b);
  }
}

function paintConflict() {
  const c = song.view && song.view.conflict;
  $("conflict").hidden = !c;
  if (c) {
    $("cfText").innerHTML = `You have <b>${esc(song.view.meta.name)}</b> open, your partner has <b>${esc(c.name)}</b>. Pick one to work on together (the other one stays saved).`;
    $("cfTheirs").textContent = `Open "${c.name}"`;
    $("cfMine").textContent = `Send "${song.view.meta.name}"`;
  }
}

// ---------- demo backend (page in a normal browser) ----------
function demoSong() {
  const me = "me", partner = "partner";
  const peaks = (secs, seed, sr = 48000) => {
    const n = Math.floor(secs * sr / 512), a = new Int8Array(2 * n);
    for (let i = 0; i < n; i++) {
      const t = i * 512 / sr, env = 0.35 + 0.3 * Math.sin(t * 1.3 + seed) * Math.sin(t * 0.21 + seed * 2);
      const beat = Math.exp(-((t * (seed + 1.5)) % 1) * 5);
      const v = Math.min(1, Math.abs(env) * 0.6 + beat * 0.45) * 120 * (0.75 + 0.25 * Math.sin(i * 0.7 + seed));
      a[2 * i] = -Math.round(v); a[2 * i + 1] = Math.round(v * 0.92);
    }
    let s = ""; for (const b of new Uint8Array(a.buffer)) s += String.fromCharCode(b);
    return btoa(s);
  };
  const files = {
    "take-g1.wav": { frames: 48000 * 16, sr: 48000, owner: me, here: true, mine: true },
    "take-v1.wav": { frames: 48000 * 12, sr: 48000, owner: partner, here: true, mine: false },
    "take-k1.wav": { frames: 48000 * 8, sr: 48000, owner: partner, here: false, mine: false },
  };
  const pk = { "take-g1.wav": peaks(16, 0.3), "take-v1.wav": peaks(12, 1.7) };
  const view = {
    pid: "pdemo", meta: { name: "Summer jam", bpm: 96, beats: 4, loop: true, loop_start: 8, loop_end: 24 },
    tracks: [
      { id: "t1", name: "Guitar", order: 1, owner: me, src: 0, arm: true, vol: 0.8, pan: 0, rack: [{ kind: "reverb" }], slot: 0, mine: true, mute: false, solo: false },
      { id: "t2", name: "Vocals", order: 2, owner: partner, src: 0, arm: true, vol: 0.75, pan: 0, rack: [], slot: 1, mine: false, mute: false, solo: false },
      { id: "t3", name: "Keys", order: 3, owner: partner, src: 1, arm: false, vol: 0.7, pan: 0, rack: [], slot: 2, mine: false, mute: false, solo: false },
    ],
    clips: [
      { id: "c1", track: "t1", file: "take-g1.wav", start: 0, offset: 0, len: 15, gain: 1 },
      { id: "c2", track: "t2", file: "take-v1.wav", start: 8, offset: 0.5, len: 11, gain: 1 },
      { id: "c3", track: "t3", file: "take-k1.wav", start: 16, offset: 0, len: 8, gain: 1 },
    ],
    files, me, partner, local: { click: true, countin: true }, conflict: null, shared: true,
  };
  let v = 1, playing = false, recording = false, recFrom = null, pos = 0, t0 = 0;
  const copy = () => JSON.parse(JSON.stringify(view));
  const now = () => (playing ? pos + (performance.now() - t0) / 1000 / (60 / view.meta.bpm) : pos);
  return {
    daw_project: async () => copy(),
    daw_edit: async (changes) => {
      for (const [kind, id, f] of changes) {
        if (kind === "meta") Object.assign(view.meta, f);
        const list = kind === "track" ? view.tracks : kind === "clip" ? view.clips : null;
        if (!list) continue;
        const o = list.find((x) => x.id === id);
        if (f.del) { if (o) list.splice(list.indexOf(o), 1); continue; }
        if (o) Object.assign(o, f); else list.push({ id, ...f, mine: f.owner === me, slot: list.length });
      }
      v++; return copy();
    },
    daw_local: async (k, val, tid) => {
      if (tid) view.tracks.find((t) => t.id === tid)[k] = val; else view.local[k] = val;
      v++; return copy();
    },
    daw_play: async () => { pos = now(); playing = true; t0 = performance.now(); },
    daw_stop: async () => { playing = false; recording = false; pos = 0; },
    daw_record: async () => { playing = true; recording = true; recFrom = pos; t0 = performance.now(); return { ok: true }; },
    daw_locate: async (p) => { pos = p; t0 = performance.now(); },
    daw_peaks: async (n) => pk[n] || null,
    daw_projects: async () => [{ pid: "pdemo", name: view.meta.name, modified: Date.now() / 1000, clips: view.clips.length, current: true },
      { pid: "p2", name: "Blues in A", modified: Date.now() / 1000 - 86400, clips: 5, current: false }],
    daw_new: async () => copy(), daw_open: async () => copy(), daw_resolve: async () => copy(),
    status: () => ({ v, pos: playing ? pos + (performance.now() - t0) / 1000 / (60 / view.meta.bpm) : pos, playing, recording, recFrom,
      levels: playing ? { t1: 0.4, t2: 0.25 } : {}, xfer: { "take-k1.wav": [3_100_000, 4_600_000, "in"] }, notices: [] }),
  };
}
