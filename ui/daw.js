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
// pattern clips are measured in beats (blen), audio clips in seconds (len)
const clipBeats = (c) => (c.pattern ? c.blen ?? 4 : c.len / spb());
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
    // a bus is a track too, kept apart from the playlist's
    const bus = kind === "track" && (f.bus || (v.buses || []).some((b) => b.id === id));
    const list = { track: bus ? (v.buses ||= []) : v.tracks, clip: v.clips, pattern: (v.patterns ||= []), note: (v.notes ||= []) }[kind];
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
  if (ui.sel[0] === "T" && !trackBySlot(+ui.sel.slice(1))) { ui.sel = "L0"; renderRack(); }
  // track strips in the mixer follow the song's tracks (names, owners, slots),
  // and their racks when those changed elsewhere (the partner, an undo); a knob
  // you're turning yourself already matches, so it isn't redrawn under you
  // (not the sends: a send knob being turned would be rebuilt under the pointer)
  const sig = JSON.stringify([...v.tracks, ...(v.buses || [])].map((t) => [t.id, t.slot, t.name, t.mine, t.out]));
  const rs = (r) => JSON.stringify((r || []).map((x) => [x.kind, !!x.on, x.model || null, x.vst?.cid || null,
    ...(FXMETA[x.kind]?.knobs || []).map(([k]) => Math.round((x[k] ?? 0) * 1000))]));
  const racksMoved = ui.mixer?.tracks && ([...v.tracks, ...(v.buses || [])].some((t) => t.slot != null && rs(t.rack) !== rs(ui.mixer.tracks[t.slot].rack))
    || rs(v.meta.master_rack) !== rs(ui.mixer.master_strip?.rack));
  if (!before || sig !== song.trackSig || racksMoved) {
    song.trackSig = sig;
    api.mixer().then((m) => { ui.mixer = m; renderStrips(); if (ui.sel[0] === "T") renderRack(); });
  }
  songRender();
  seqRender();
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
  // snap: a small magnet button with a menu
  const paintSnap = () => {
    const label = (SNAPS.find(([, v]) => v === song.snap) || SNAPS[1])[0];
    $("snapLbl").textContent = label;
    $("snapBtn").title = `Snap to grid: ${label} (hold Alt to move freely)`;
  };
  paintSnap();
  $("snapBtn").onclick = () => {
    const r = $("snapBtn").getBoundingClientRect();
    showCtx(SNAPS.map(([label, v]) => [(v === song.snap ? "✓  " : "     ") + "Snap: " + label, "", () => { song.snap = v; store.set("songSnap", v); paintSnap(); }]), r.left, r.bottom + 4);
  };
  $("zoomIn").onclick = () => zoomBy(1.4);
  $("zoomOut").onclick = () => zoomBy(1 / 1.4);
  newTrackSetup();

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
  lanes.addEventListener("pointermove", lanesHover);
  lanes.addEventListener("pointerleave", () => { if (song.hover) { song.hover = null; song.hoverKey = ""; songDraw(); } });
  lanes.addEventListener("dblclick", lanesDbl);
  lanes.addEventListener("contextmenu", lanesContext);
  document.addEventListener("pointerdown", (e) => { if (!e.target.closest("#ctxMenu")) hideCtx(); });
  $("undoBtn").onclick = undo;
  $("redoBtn").onclick = redo;
  $("exGo").onclick = runExport;
  $("exX").onclick = () => ($("exportm").hidden = true);
  $("exportm").onclick = (e) => e.target === $("exportm") && ($("exportm").hidden = true);
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
  ui.view = ["mixer", "seq"].includes(v) ? v : "song";
  store.set("view", ui.view);
  $("viewSong").hidden = ui.view !== "song";
  $("viewSeq").hidden = ui.view !== "seq";
  $("viewMixer").hidden = ui.view !== "mixer";
  for (const b of $("viewSeg").querySelectorAll("button")) b.classList.toggle("on", b.dataset.v === ui.view);
  if (ui.view === "song") requestAnimationFrame(songDraw);
  if (ui.view === "seq") requestAnimationFrame(seqRender);
  if (typeof syncMidiTarget === "function") syncMidiTarget();
}

function songKeys(e) {
  if (ui.view === "seq" || e.target.closest("input, select, textarea") || !$("vstm").hidden || !$("newTrackM").hidden) return;
  if (e.target.closest("button")) e.target.blur(); // or space would press that button too
  // the transport is in the top bar, so it answers in the mixer too
  if (e.code === "Space") { e.preventDefault(); togglePlay(); }
  else if (e.key === "r" || e.key === "R") { e.preventDefault(); toggleRec(); }
  else if (e.key === "Home") { e.preventDefault(); api.daw_locate(0); }
  else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "e") { e.preventDefault(); openExport(); }
  else if (ui.view !== "song") return;
  else if ((e.key === "Delete" || e.key === "Backspace") && song.sel.size) { e.preventDefault(); deleteSel(); }
  else if (e.key === "Escape") { hideCtx(); song.sel.clear(); songDraw(); }
  else if (e.ctrlKey || e.metaKey) {
    const k = e.key.toLowerCase();
    const act = { a: () => { song.view.clips.forEach((c) => song.sel.add(c.id)); songDraw(); },
      z: () => (e.shiftKey ? redo() : undo()), y: redo, d: duplicateSel, c: () => copySel(), x: () => copySel(true), v: () => paste() }[k];
    if (act) { e.preventDefault(); act(); }
  } else if (e.key === "s" || e.key === "S") { e.preventDefault(); splitAtPlayhead(); }
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
  herePaint(d.partnerHere || null, d.partnerNotes || {});
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
  $("undoBtn").disabled = !v.undo;
  $("redoBtn").disabled = !v.redo;
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
    arm.title = t.inst ? "Record the notes you play on this channel" : t.mine ? "Record on this track (from your input)" : "Record on this track (from your partner's input, on their side)";
    arm.onclick = () => songEdit([["track", t.id, { arm: !t.arm }]]);
    const del = document.createElement("button");
    del.className = "del"; del.textContent = "✕"; del.title = "Delete this track";
    del.onclick = () => deleteTrack(t);
    btns.append(arm,
      toggleBtn("M", "m", t.mute, "Mute (just for you)", (on) => api.daw_local("mute", on, t.id).then(setSongView)),
      toggleBtn("S", "s", t.solo, "Solo (just for you)", (on) => api.daw_local("solo", on, t.id).then(setSongView)), del);

    const row2 = document.createElement("div");
    row2.className = "row2";
    let src;
    if (t.inst) {
      // a channel: it plays its instrument (notes from patterns), it has no input
      src = document.createElement("button");
      src.className = "tsrc chanlbl";
      src.textContent = "♪ " + (t.inst.kind === "vst" ? t.inst.vst?.name || "VST" : INST[t.inst.kind]?.name || "Instrument");
      src.title = "A channel: open it in the sequencer";
      src.onclick = () => { setMainView("seq"); selectChannel(t.id); };
    } else {
      src = document.createElement("select");
      src.className = "tsrc"; src.title = "Who plays on this track, and from which input";
      const { opts, cur } = srcOptions(t);
      for (const [val, label] of opts) src.append(new Option(label, val, false, val === cur));
      src.onfocus = () => (song.headBusy = true);
      src.onblur = () => { song.headBusy = false; if (song.pendingHeads) renderHeads(); };
      src.onchange = () => {
        const [owner, s] = src.value.split("|");
        songEdit([["track", t.id, { owner, src: +s }]]);
      };
    }
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
  hereMarks();
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

// fader and knob drags send at most ~15 edits a second
const pendingEdits = {};
function throttledEdit(kind, id, fields) {
  const k = kind + "\n" + id;
  pendingEdits[k] = { ...(pendingEdits[k] || {}), ...fields };
  if (throttledEdit.t) return;
  throttledEdit.t = setTimeout(() => {
    throttledEdit.t = null;
    const ch = Object.entries(pendingEdits).map(([key, f]) => [...key.split("\n"), f]);
    for (const key of Object.keys(pendingEdits)) delete pendingEdits[key];
    songEdit(ch);
  }, 66);
}
const songEditThrottled = (tid, fields) => throttledEdit("track", tid, fields);

// ---------- new track: a software instrument, or a line in ----------
// From the playlist an instrument gets a pattern right away, already in the song
// at the playhead; from the sequencer (+ Channel) it joins the open pattern.
const nt = { kind: null, opt: null, from: "song", named: false, arm: true };

function newTrackSetup() {
  $("addTrack").onclick = () => openNewTrack("song");
  for (const b of document.querySelectorAll(".ntkind")) b.onclick = () => ntKind(b.dataset.k);
  $("ntName").oninput = () => (nt.named = true);
  $("ntArm").onclick = () => $("ntArm").classList.toggle("on", (nt.arm = !nt.arm));
  $("ntGo").onclick = ntCreate;
  $("ntX").onclick = () => ($("newTrackM").hidden = true);
  $("newTrackM").onclick = (e) => e.target === $("newTrackM") && ($("newTrackM").hidden = true);
  $("newTrackM").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && nt.opt) { e.preventDefault(); ntCreate(); }
    if (e.key === "Escape") $("newTrackM").hidden = true;
  });
}

function openNewTrack(from = "song", kind = null) {
  nt.from = from; nt.named = false; nt.arm = true;
  $("ntName").value = "";
  $("newTrackM").hidden = false;
  ntKind(kind);
  if (!kind) document.querySelector(".ntkind").focus();
}

function ntKind(kind) {
  nt.kind = kind; nt.opt = null;
  for (const b of document.querySelectorAll(".ntkind")) b.classList.toggle("on", b.dataset.k === kind);
  const box = $("ntOpts");
  box.innerHTML = "";
  $("ntFoot").hidden = !kind;
  if (!kind) return;
  const opts = kind === "inst"
    ? [["drums", INST.drums.name, false], ["synth", INST.synth.name, false], ["vst", "VST instrument…", false]]
    : audioSources();
  for (const [val, label, partner] of opts) {
    const b = document.createElement("button");
    b.className = "ntopt" + (partner ? " partner" : "");
    b.textContent = label; b.dataset.v = val;
    b.onclick = () => ntPick(val, label);
    b.ondblclick = ntCreate;
    box.append(b);
  }
  ntPick(opts[0][0], opts[0][1]);
  $("ntArm").hidden = kind !== "audio";
  $("ntArm").classList.toggle("on", nt.arm);
  $("ntGo").focus();
}

// your inputs, then your partner's channels
function audioSources() {
  const v = song.view, out = [];
  for (const [i, s] of (ui.mixer?.local || []).entries()) if (s.active) out.push([`${v.me}|${i}`, `You · ${stripName("L" + i)}`, false]);
  if (v.partner) for (const s of ui.partnerStrips || []) out.push([`${v.partner}|${s.i}`, `Partner · ${s.name}`, true]);
  if (!out.length) out.push([`${v.me}|0`, "You · In 1", false]);
  return out;
}

function ntPick(val, label) {
  nt.opt = val;
  for (const b of $("ntOpts").children) b.classList.toggle("on", b.dataset.v === val);
  if (!nt.named) $("ntName").value = val === "vst" ? "" : uniqueTrackName(nt.kind === "audio" ? label.split(" · ").pop() : label);
}

function uniqueTrackName(base) {
  const names = new Set(song.view.tracks.map((t) => t.name));
  if (!names.has(base)) return base;
  let n = 2;
  while (names.has(`${base} ${n}`)) n++;
  return `${base} ${n}`;
}

function ntCreate() {
  if (!nt.opt) return;
  const name = $("ntName").value.trim();
  $("newTrackM").hidden = true;
  if (nt.kind === "inst") {
    if (nt.opt === "vst") {
      ui.instFor = nt.from === "song" ? "new-track" : "new-channel";
      ui.instName = name || null;
      openVstBrowser("inst");
      return;
    }
    createInstrumentTrack({ kind: nt.opt, ...(INST_DEFAULTS[nt.opt] || {}) }, name || uniqueTrackName(INST[nt.opt].name), nt.from === "song");
    return;
  }
  const v = song.view;
  const [owner, s] = nt.opt.split("|");
  const order = v.tracks.reduce((a, t) => Math.max(a, t.order || 0), 0) + 1;
  songEdit([["track", newId("t"), { name: name || `Track ${v.tracks.length + 1}`, order, owner, src: +s, arm: nt.arm, vol: 0.8, pan: 0, rack: [] }]]);
  requestAnimationFrame(() => ($("lanes").scrollTop = 1e6));
}

function deleteTrack(t) {
  const clips = song.view.clips.filter((c) => c.track === t.id);
  if (clips.length && !confirm(`Delete "${t.name}" and its ${clips.length} clip${clips.length === 1 ? "" : "s"}? This deletes it for your partner too.`)) return;
  songEdit([["track", t.id, { del: true }], ...clips.map((c) => ["clip", c.id, { del: true }])]);
}

function trackBySlot(slot) {
  return song.view ? [...song.view.tracks, ...(song.view.buses || [])].find((t) => t.slot === slot) : null;
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
  // clips (while dragging: where they'd end up; a copy-drag leaves the originals)
  const d = song.drag, res = d && d.moved ? d.result || {} : {};
  const place = (c, ghost) => {
    const row = ghost && ghost.row != null ? ghost.row : trackRow(c.track);
    if (row < 0) return;
    const cc = ghost ? { ...c, ...ghost } : c;
    const x = cc.start * ppb - sx, cw = clipBeats(cc) * ppb, y = row * ROW - sy;
    if (x > w || x + cw < 0 || y > h || y + ROW < 0) return;
    drawClip(ctx, cc, x, y + 2, cw, ROW - 5, v.tracks[row]);
  };
  for (const c of v.clips) place(c, d && !d.copy ? res[c.id] : null);
  if (d && d.copy) for (const c of v.clips) if (res[c.id]) place(c, res[c.id]);
  drawRuler();
  drawOverlay();
}

function drawClip(ctx, c, x, y, w, h, track) {
  if (c.pattern) { drawPatternClip(ctx, c, x, y, w, h); return; }
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
  // fades: shade what's faded out, a line where the level goes
  const fin = (c.fade_in || 0) / spb() * song.ppb, fout = (c.fade_out || 0) / spb() * song.ppb;
  ctx.fillStyle = "rgba(8,7,11,0.55)";
  ctx.strokeStyle = "#ffffffb0";
  ctx.lineWidth = 1;
  if (fin > 0.5) {
    ctx.beginPath(); ctx.moveTo(x, y + 15); ctx.lineTo(x + fin, y + 15); ctx.lineTo(x, y + h); ctx.closePath(); ctx.fill();
    ctx.beginPath(); ctx.moveTo(x, y + h); ctx.lineTo(x + fin, y + 15); ctx.stroke();
  }
  if (fout > 0.5) {
    ctx.beginPath(); ctx.moveTo(x + w, y + 15); ctx.lineTo(x + w - fout, y + 15); ctx.lineTo(x + w, y + h); ctx.closePath(); ctx.fill();
    ctx.beginPath(); ctx.moveTo(x + w, y + h); ctx.lineTo(x + w - fout, y + 15); ctx.stroke();
  }
  ctx.fillStyle = "#121016";
  ctx.font = "700 10.5px system-ui, sans-serif";
  const g = c.gain ?? 1;
  const label = (track ? track.name : "Clip") + (Math.abs(g - 1) > 0.001 ? `  ${fmtDb(g)}` : "");
  ctx.fillText(label, Math.max(x, 0) + 6, y + 11);
  // fade handles on the clip you're pointing at (or that's selected)
  if ((song.hover === c.id || sel) && w > 30) {
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(x + Math.max(fin, 5) - 3.5, y + 1, 7, 7);
    ctx.fillRect(x + Math.min(w - fout, w - 5) - 3.5, y + 1, 7, 7);
  }
  ctx.restore();
  ctx.beginPath();
  ctx.roundRect(x + 0.5, y + 0.5, Math.max(2, w - 1), h - 1, r);
  ctx.strokeStyle = sel ? "#ffffff" : col + "c0";
  ctx.lineWidth = sel ? 1.5 : 1;
  ctx.stroke();
}

// a pattern block: its name and a sketch of its notes, repeated to fill it
function drawPatternClip(ctx, c, x, y, w, h) {
  const p = (song.view.patterns || []).find((q) => q.id === c.pattern);
  const col = patColor(c.pattern), sel = song.sel.has(c.id);
  ctx.save();
  ctx.beginPath();
  ctx.roundRect(x + 0.5, y + 0.5, Math.max(2, w - 1), h - 1, 6);
  ctx.fillStyle = "rgba(20,18,26,0.92)";
  ctx.fill();
  ctx.clip();
  ctx.fillStyle = col;
  ctx.globalAlpha = sel ? 0.95 : 0.7;
  ctx.fillRect(x, y, w, 15);
  ctx.globalAlpha = 1;
  if (p) {
    const notes = (song.view.notes || []).filter((n) => n.pattern === p.id);
    if (notes.length) {
      const plen = p.len || 4 * bpb(), boff = c.boff || 0;
      let lo = 127, hi = 0;
      for (const n of notes) { lo = Math.min(lo, n.key); hi = Math.max(hi, n.key); }
      const span = Math.max(6, hi - lo + 1), top = y + 18, hh = h - 21;
      ctx.fillStyle = col;
      const lane = $("lanes").clientWidth;
      for (let k = Math.floor(boff / plen); (k * plen - boff) < clipBeats(c); k++) {
        for (const n of notes) {
          const b = k * plen + n.start - boff;
          if (b < 0 || b >= clipBeats(c) || n.start >= plen) continue;
          const nx = x + b * song.ppb;
          if (nx > lane || nx < -40) continue;
          const nw = Math.max(1.5, Math.min(n.len, clipBeats(c) - b) * song.ppb - 0.5);
          ctx.fillRect(nx, top + (hi - n.key) / span * (hh - 3), nw, Math.max(2, hh / span - 0.5));
        }
      }
    }
  }
  ctx.fillStyle = "#121016";
  ctx.font = "700 10.5px system-ui, sans-serif";
  ctx.fillText(p ? p.name || "Pattern" : "(pattern gone)", Math.max(x, 0) + 6, y + 11);
  ctx.restore();
  ctx.beginPath();
  ctx.roundRect(x + 0.5, y + 0.5, Math.max(2, w - 1), h - 1, 6);
  ctx.strokeStyle = sel ? "#ffffff" : col;
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
  hereDrawRuler(ctx, sx);
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
  const b = song.band;
  if (b && b.moved) {
    const x0 = Math.min(b.b0, b.b1) * ppb - sx, x1 = Math.max(b.b0, b.b1) * ppb - sx;
    const y0 = Math.min(b.y0, b.y1) - sy, y1 = Math.max(b.y0, b.y1) - sy;
    ctx.fillStyle = "rgba(239,234,244,0.07)";
    ctx.fillRect(x0, y0, x1 - x0, y1 - y0);
    ctx.strokeStyle = "rgba(239,234,244,0.6)";
    ctx.strokeRect(x0 + 0.5, y0 + 0.5, x1 - x0, y1 - y0);
  }
  hereDrawSong(ctx, sx, sy, w, h);
}

let lastFrameDraw = 0;
function songFrame() {
  seqFrame();
  hereFrame();
  if (!song.view) return;
  const st = song.st;
  const pos = curPos();
  // the LCD is in the top bar: it runs in every view
  $("tpPos").textContent = fmtPos(pos);
  $("tpTime").textContent = fmtTime(pos);
  if (ui.view !== "song") return;
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
// Over a clip: the body moves it (ctrl/cmd: a copy), the left/right edge trims
// it, the small handles in its top corners set the fades. Alt: no snapping.
// On empty space: click moves the playhead, drag selects a box of clips.
const EDGE = 6;
const CURSORS = { body: "grab", left: "w-resize", right: "e-resize", fin: "col-resize", fout: "col-resize" };

function lanePoint(e) {
  const lanes = $("lanes"), r = lanes.getBoundingClientRect();
  const x = e.clientX - r.left, y = e.clientY - r.top;
  const px = x + lanes.scrollLeft, py = y + lanes.scrollTop;
  return { inside: x >= 0 && y >= 0 && x <= lanes.clientWidth && y <= lanes.clientHeight, px, py, beat: px / song.ppb, row: Math.floor(py / ROW) };
}

function hitClip(beat, row) {
  const v = song.view, t = v.tracks[row];
  if (!t) return null;
  for (let i = v.clips.length - 1; i >= 0; i--) {
    const c = v.clips[i];
    if (c.track === t.id && beat >= c.start && beat <= c.start + clipBeats(c)) return c;
  }
  return null;
}

function hitZone(p) {
  const c = hitClip(p.beat, p.row);
  if (!c) return null;
  const ppb = song.ppb, w = clipBeats(c) * ppb, lx = p.px - c.start * ppb, ly = p.py - (p.row * ROW + 2);
  const fin = (c.fade_in || 0) / spb() * ppb, fout = (c.fade_out || 0) / spb() * ppb;
  if (ly < 14 && w > 30 && !c.pattern) {
    if (Math.abs(lx - Math.max(fin, 5)) <= 6 && lx < w / 2) return { c, zone: "fin" };
    if (Math.abs(lx - Math.min(w - fout, w - 5)) <= 6 && lx >= w / 2) return { c, zone: "fout" };
  }
  if (w > 3 * EDGE && lx <= EDGE) return { c, zone: "left" };
  if (w > 3 * EDGE && lx >= w - EDGE) return { c, zone: "right" };
  return { c, zone: "body" };
}

const clipFields = (c) => (c.pattern ? { track: c.track, pattern: c.pattern, start: c.start, blen: c.blen, boff: c.boff || 0 }
  : { track: c.track, file: c.file, start: c.start, offset: c.offset || 0, len: c.len,
      gain: c.gain ?? 1, fade_in: c.fade_in || 0, fade_out: c.fade_out || 0 });
const fileDur = (c) => { const f = song.view.files[c.file]; return f && f.frames ? f.frames / f.sr : Infinity; };
const gridOf = (mode) => (mode < 0 ? 0 : mode === 0 ? bpb() : mode);
const selClips = () => song.view.clips.filter((c) => song.sel.has(c.id));
const MIN_LEN = 0.05; // seconds

function lanesHover(e) {
  if (song.drag || song.band || e.buttons || !song.view) return;
  const p = lanePoint(e), h = p.inside ? hitZone(p) : null;
  $("lanes").style.cursor = h ? CURSORS[h.zone] : "";
  const key = h ? h.c.id + h.zone : "";
  if (key !== song.hoverKey) { song.hoverKey = key; song.hover = h ? h.c.id : null; songDraw(); }
}

function lanesDown(e) {
  if (e.button !== 0 || !song.view) return;
  const p = lanePoint(e);
  if (!p.inside) return; // the scrollbars
  hideCtx();
  const lanes = $("lanes");
  lanes.setPointerCapture(e.pointerId);
  const h = hitZone(p);
  if (!h) { bandSelect(e, p); return; }
  const c = h.c;
  if (e.shiftKey && h.zone === "body") { song.sel.has(c.id) ? song.sel.delete(c.id) : song.sel.add(c.id); songDraw(); return; }
  if (!song.sel.has(c.id)) { song.sel.clear(); song.sel.add(c.id); }
  const d = { zone: h.zone, x0: e.clientX, y0: e.clientY, moved: false, anchor: c, row0: p.row, grab: p.beat - c.start,
    copy: (e.ctrlKey || e.metaKey) && h.zone === "body", sel: h.zone === "body" ? selClips() : [c], result: null };
  song.drag = d;
  if (h.zone === "body") lanes.style.cursor = "grabbing";
  const move = (ev) => {
    if (!d.moved && Math.hypot(ev.clientX - d.x0, ev.clientY - d.y0) < 4) return;
    d.moved = true;
    d.result = dragResult(d, lanePoint(ev), ev.altKey ? -1 : song.snap);
    songDraw();
  };
  const up = () => {
    lanes.removeEventListener("pointermove", move);
    lanes.removeEventListener("pointerup", up);
    song.drag = null;
    lanes.style.cursor = "";
    if (d.moved && d.result) commitDrag(d);
    songDraw();
  };
  lanes.addEventListener("pointermove", move);
  lanes.addEventListener("pointerup", up);
  songDraw();
}

// what a drag would do: {id: {fields, row}} for the clips it changes
function dragResult(d, q, snap) {
  const v = song.view, s = spb(), c = d.anchor, out = {};
  const clampRow = (r) => Math.max(0, Math.min(v.tracks.length - 1, r));
  if (d.zone === "body") {
    const db = Math.max(0, snapBeat(q.beat - d.grab, snap)) - c.start;
    const minStart = Math.min(...d.sel.map((k) => k.start));
    const shift = Math.max(db, -minStart);
    const dr = q.row - d.row0;
    for (const k of d.sel) out[k.id] = { start: k.start + shift, row: clampRow(trackRow(k.track) + dr) };
  } else if (c.pattern && d.zone === "left") {
    // a pattern clip can start anywhere in its pattern (and wraps round)
    const hi = c.start + clipBeats(c) - STEP;
    const start = Math.min(hi, Math.max(0, snapBeat(q.beat, snap)));
    out[c.id] = { start, boff: Math.max(0, (c.boff || 0) + start - c.start), blen: c.blen - (start - c.start) };
  } else if (c.pattern && d.zone === "right") {
    const end = Math.max(c.start + STEP, snapBeat(q.beat, snap));
    out[c.id] = { blen: end - c.start };
  } else if (d.zone === "left") {
    const lo = Math.max(0, c.start - (c.offset || 0) / s), hi = c.start + clipBeats(c) - MIN_LEN / s;
    const start = Math.min(hi, Math.max(lo, snapBeat(q.beat, snap)));
    const dt = (start - c.start) * s, len = c.len - dt;
    out[c.id] = { start, offset: (c.offset || 0) + dt, len, fade_in: Math.min(c.fade_in || 0, len - (c.fade_out || 0)) };
  } else if (d.zone === "right") {
    const lo = c.start + MIN_LEN / s, hi = c.start + (fileDur(c) - (c.offset || 0)) / s;
    const end = Math.min(hi, Math.max(lo, snapBeat(q.beat, snap)));
    const len = (end - c.start) * s;
    out[c.id] = { len, fade_out: Math.min(c.fade_out || 0, len - (c.fade_in || 0)) };
  } else if (d.zone === "fin") {
    out[c.id] = { fade_in: Math.max(0, Math.min(c.len - (c.fade_out || 0), (q.beat - c.start) * s)) };
  } else if (d.zone === "fout") {
    out[c.id] = { fade_out: Math.max(0, Math.min(c.len - (c.fade_in || 0), (c.start + clipBeats(c) - q.beat) * s)) };
  }
  return out;
}

function commitDrag(d) {
  const v = song.view, ch = [];
  if (d.copy) {
    song.sel.clear();
    for (const k of d.sel) {
      const r = d.result[k.id], id = newId("c");
      ch.push(["clip", id, { ...clipFields(k), start: r.start, track: v.tracks[r.row].id }]);
      song.sel.add(id);
    }
  } else {
    for (const [id, r] of Object.entries(d.result)) {
      const { row, ...f } = r;
      if (row != null) f.track = v.tracks[row].id;
      ch.push(["clip", id, f]);
    }
  }
  songEdit(ch);
}

// drag on empty space: select every clip the box touches; a plain click moves the playhead
function bandSelect(e, p) {
  const lanes = $("lanes");
  const keep = e.shiftKey ? new Set(song.sel) : new Set();
  song.band = { b0: p.beat, y0: p.py, b1: p.beat, y1: p.py, moved: false };
  const move = (ev) => {
    const q = lanePoint(ev), b = song.band;
    b.b1 = q.beat; b.y1 = q.py;
    if (!b.moved && Math.abs((b.b1 - b.b0) * song.ppb) + Math.abs(b.y1 - b.y0) < 5) return;
    b.moved = true;
    const [lb, hb] = [Math.min(b.b0, b.b1), Math.max(b.b0, b.b1)], [ly, hy] = [Math.min(b.y0, b.y1), Math.max(b.y0, b.y1)];
    song.sel = new Set(keep);
    for (const c of song.view.clips) {
      const row = trackRow(c.track);
      if (row * ROW + ROW > ly && row * ROW < hy && c.start + clipBeats(c) > lb && c.start < hb) song.sel.add(c.id);
    }
    songDraw();
  };
  const up = () => {
    lanes.removeEventListener("pointermove", move);
    lanes.removeEventListener("pointerup", up);
    const b = song.band;
    song.band = null;
    if (!b.moved) {
      if (!e.shiftKey) song.sel.clear();
      const at = Math.max(0, snapBeat(p.beat));
      api.daw_locate(at);
      if (song.st && !song.st.playing) song.st.pos = at;
    }
    songDraw();
  };
  lanes.addEventListener("pointermove", move);
  lanes.addEventListener("pointerup", up);
}

function lanesDbl(e) {
  // double-click: a pattern opens in the sequencer; empty space gets the
  // current pattern; an audio clip selects everything on its track from there on
  const p = lanePoint(e), c = hitClip(p.beat, p.row);
  if (!c) { insertPattern(p); return; }
  if (c.pattern) { seq.pat = c.pattern; setMainView("seq"); return; }
  for (const k of song.view.clips) if (k.track === c.track && k.start >= c.start) song.sel.add(k.id);
  songDraw();
}

function insertPattern(p) {
  const pat = curPattern(), t = song.view.tracks[p.row];
  if (!pat || !t) return;
  const at = Math.max(0, Math.floor(p.beat / bpb() + 1e-9) * bpb());
  const id = newId("c");
  song.sel = new Set([id]);
  songEdit([["clip", id, { track: t.id, pattern: pat.id, start: at, blen: patLen(pat), boff: 0 }]]);
}

// ---------- editing: split, duplicate, copy/paste, delete ----------
function splitClips(beat, clips) {
  const s = spb(), ch = [];
  for (const c of clips) {
    if (beat <= c.start + 1e-6 || beat >= c.start + clipBeats(c) - 1e-6) continue;
    if (c.pattern) {
      const lb = beat - c.start;
      ch.push(["clip", c.id, { blen: lb }]);
      ch.push(["clip", newId("c"), { ...clipFields(c), start: beat, boff: (c.boff || 0) + lb, blen: c.blen - lb }]);
      continue;
    }
    const left = (beat - c.start) * s, right = c.len - left;
    ch.push(["clip", c.id, { len: left, fade_out: 0, fade_in: Math.min(c.fade_in || 0, left) }]);
    ch.push(["clip", newId("c"), { ...clipFields(c), start: beat, offset: (c.offset || 0) + left, len: right,
      fade_in: 0, fade_out: Math.min(c.fade_out || 0, right) }]);
  }
  if (ch.length) songEdit(ch);
  return ch.length > 0;
}

function splitAtPlayhead() {
  const b = curPos(), crosses = (c) => b > c.start && b < c.start + clipBeats(c);
  const sel = selClips().filter(crosses);
  if (!splitClips(b, sel.length ? sel : song.view.clips.filter(crosses))) toast("Put the playhead on a clip to split it there (S).");
}

function duplicateSel() {
  const sel = selClips();
  if (!sel.length) return;
  const s0 = Math.min(...sel.map((c) => c.start)), e0 = Math.max(...sel.map((c) => c.start + clipBeats(c)));
  const g = gridOf(song.snap);
  const span = g ? Math.max(g, Math.ceil((e0 - s0) / g - 1e-6) * g) : e0 - s0;
  const ch = [];
  song.sel.clear();
  for (const c of sel) {
    const id = newId("c");
    ch.push(["clip", id, { ...clipFields(c), start: c.start + span }]);
    song.sel.add(id);
  }
  songEdit(ch);
}

function deleteSel() {
  const ids = [...song.sel];
  song.sel.clear();
  if (ids.length) songEdit(ids.map((id) => ["clip", id, { del: true }]));
}

function copySel(cut = false) {
  const sel = selClips();
  if (!sel.length) return;
  const s0 = Math.min(...sel.map((c) => c.start));
  const rows = sel.map((c) => trackRow(c.track));
  song.clipboard = { minRow: Math.min(...rows), items: sel.map((c, i) => ({ ...clipFields(c), rel: c.start - s0, row: rows[i] })) };
  if (cut) deleteSel();
  else toast(`Copied ${sel.length} clip${sel.length === 1 ? "" : "s"}. Ctrl+V pastes them at the playhead.`);
}

// at: beats (default the playhead); row: the track the top clip goes on (default: their own tracks)
function paste(at, row) {
  const cb = song.clipboard, v = song.view;
  if (!cb) return;
  const base = at ?? Math.max(0, snapBeat(curPos()));
  const ch = [];
  song.sel.clear();
  for (const it of cb.items) {
    const r = row == null ? null : row + (it.row - cb.minRow);
    const track = r == null ? it.track : v.tracks[Math.min(v.tracks.length - 1, Math.max(0, r))]?.id;
    if (!track || !v.tracks.some((t) => t.id === track)) continue;
    const { rel, row: _r, ...f } = it;
    const id = newId("c");
    ch.push(["clip", id, { ...f, track, start: base + rel }]);
    song.sel.add(id);
  }
  songEdit(ch);
}

async function undo() { setSongView(await api.daw_undo()); }
async function redo() { setSongView(await api.daw_redo()); }

// ---------- right-click menu ----------
function lanesContext(e) {
  e.preventDefault();
  if (!song.view) return;
  const p = lanePoint(e);
  if (!p.inside) return;
  const h = hitZone(p), items = [];
  if (h) {
    const c = h.c;
    if (!song.sel.has(c.id)) { song.sel.clear(); song.sel.add(c.id); songDraw(); }
    const at = snapBeat(p.beat);
    const n = song.sel.size;
    items.push(["Split here", "S", () => splitClips(at > c.start && at < c.start + clipBeats(c) ? at : p.beat, selClips())]);
    items.push([n > 1 ? `Duplicate ${n} clips` : "Duplicate", "Ctrl+D", duplicateSel]);
    items.push(["Copy", "Ctrl+C", () => copySel()]);
    items.push([n > 1 ? `Delete ${n} clips` : "Delete", "Del", deleteSel]);
    if (c.pattern) {
      items.push(["Open in the sequencer", "dbl-click", () => { seq.pat = c.pattern; setMainView("seq"); }]);
    } else {
      items.push(["Remove fades", "", () => songEdit(selClips().map((k) => ["clip", k.id, { fade_in: 0, fade_out: 0 }]))]);
      items.push({ gain: c });
    }
  } else {
    const pat = curPattern();
    if (pat) items.push([`Put "${pat.name}" here`, "dbl-click", () => insertPattern(p)]);
    items.push(["Paste here", "Ctrl+V", () => paste(Math.max(0, snapBeat(p.beat)), p.row), !song.clipboard]);
    items.push(["Add a track", "", addTrack]);
  }
  showCtx(items, e.clientX, e.clientY);
}

function showCtx(items, x, y) {
  const m = $("ctxMenu");
  m.innerHTML = "";
  for (const it of items) {
    if (it.gain) {
      const c = it.gain, row = document.createElement("div");
      row.className = "cgain";
      const lbl = document.createElement("span");
      lbl.textContent = `Volume ${fmtDb(c.gain ?? 1)}`;
      row.append(lbl, hslider(c.gain ?? 1, (g) => {
        lbl.textContent = `Volume ${fmtDb(g)}`;
        for (const k of selClips()) { k.gain = g; throttledEdit("clip", k.id, { gain: g }); }
        songDraw();
      }, "Clip volume (double-click: 0 dB)"));
      m.append(row);
      continue;
    }
    const [label, key, fn, disabled] = it;
    const b = document.createElement("button");
    b.innerHTML = `<span>${esc(label)}</span><kbd>${esc(key)}</kbd>`;
    b.disabled = !!disabled;
    b.onclick = () => { hideCtx(); fn(); };
    m.append(b);
  }
  m.hidden = false;
  const r = m.getBoundingClientRect();
  m.style.left = Math.min(x, innerWidth - r.width - 8) + "px";
  m.style.top = Math.min(y, innerHeight - r.height - 8) + "px";
}

function hideCtx() { const m = $("ctxMenu"); if (m) m.hidden = true; }

// ---------- export ----------
const exp = { what: "mix", range: "song" };
function openExport() {
  const m = song.view.meta;
  exp.range = m.loop ? exp.range : "song";
  const seg2 = (id, key, values) => {
    for (const b of $(id).querySelectorAll("button")) {
      b.classList.toggle("on", b.dataset.v === exp[key]);
      b.disabled = b.dataset.v === "loop" && !m.loop;
      b.onclick = () => { exp[key] = b.dataset.v; seg2(id, key, values); };
    }
  };
  seg2("exWhat", "what"); seg2("exRange", "range");
  $("exNote").textContent = m.loop ? "" : "Turn on the loop to export just that part.";
  $("exProg").hidden = true; $("exShow").hidden = true; $("exGo").disabled = false;
  $("exportm").hidden = false;
}

async function runExport() {
  $("exGo").disabled = true;
  const res = await api.daw_export({ stems: exp.what === "stems", loop: exp.range === "loop" });
  if (!res || res.cancelled) { $("exGo").disabled = false; return; }
  if (res.error) { toast(res.error, true); $("exGo").disabled = false; return; }
  $("exProg").hidden = false;
  const bar = $("exProg").firstElementChild;
  for (;;) {
    const st = await api.daw_export_status();
    bar.style.width = Math.round((st.running ? st.progress : 1) * 100) + "%";
    if (st.result) {
      if (st.result.ok) {
        const n = res.files.length, name = res.files[0].split(/[\\/]/).pop();
        $("exNote").textContent = (n > 1 ? `Saved ${n} files, with ${name}.` : `Saved ${name}.`)
          + [res.note, ...(st.result.warnings || [])].filter(Boolean).map((x) => " " + x).join("");
        $("exShow").hidden = false;
        $("exShow").onclick = () => api.show_file(res.files[0]);
      } else {
        $("exNote").textContent = "The export didn't work: " + st.result.error;
      }
      $("exGo").disabled = false;
      return;
    }
    if (!st.running && !st.result) { $("exGo").disabled = false; return; }
    await new Promise((r) => setTimeout(r, 150));
  }
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
  menu.innerHTML = `<div class="macts">
      <button id="mNew"><span>New song</span></button>
      <button id="mExport"><span>Export as WAV…</span><kbd>Ctrl+E</kbd></button>
      <button id="mSound"><span>Audio &amp; MIDI settings</span></button>
      <button id="mConnect"><span>Connect to your partner</span></button>
    </div>
    <label class="lbl-sm" for="songRename">Song name</label>
    <input type="text" id="songRename" spellcheck="false" autocomplete="off">
    <div class="who${v.shared ? " on" : ""}"><i></i>${v.shared ? "Open on both sides: you're working on it together" : "Only on this computer. Connect, and your partner opens it too."}</div>
    <div class="lbl-sm" style="margin-top:12px">Your songs</div>
    <div class="songs" id="songList"><div class="lbl-sm">Loading…</div></div>`;
  menu.hidden = false;
  const inp = $("songRename");
  inp.value = v.meta.name;
  inp.onchange = () => inp.value.trim() && songEdit([["meta", "meta", { name: inp.value.trim() }]]);
  inp.onkeydown = (e) => e.key === "Enter" && inp.blur();
  $("mExport").onclick = () => { menu.hidden = true; openExport(); };
  $("mSound").onclick = () => { menu.hidden = true; openPanel("sound"); };
  $("mConnect").onclick = () => { menu.hidden = true; openPanel("connect"); };
  $("mNew").onclick = async () => {
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
// the demo's partner: in the playlist pointing at the vocals (?demo&pseq: in the
// sequencer, on the bassline), playing the bass synth now and then
function demoPartner() {
  const tt = performance.now() / 1000, seqv = location.search.includes("pseq");
  return {
    partnerHere: seqv
      ? { view: "seq", pattern: "pb", track: "t5", sel: [], notes: ["b2", "b3"], pos: 12, ptr: { in: "roll", beat: 3 + 1.5 * Math.sin(tt * 0.7), key: 47 },
          scroll: { beat: 0, row: 0 }, roll: { beat: 0, key: 46 } }
      : { view: "song", pattern: "pb", track: "t2", sel: ["c2"], notes: [], pos: 12, ptr: { in: "song", beat: 13 + 3 * Math.sin(tt * 0.6), track: "t2" },
          scroll: { beat: 0, row: 0 }, roll: null },
    partnerNotes: Math.floor(tt / 2) % 3 === 0 ? { t5: [45, 48] } : {},
  };
}

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
    pid: "pdemo", meta: { name: "Summer jam", bpm: 96, beats: 4, loop: true, loop_start: 8, loop_end: 24,
      master_rack: [{ kind: "eq", on: true, ...FX_DEFAULTS.eq, low: 0.56, high: 0.6, lowcut: 0.12 }, { kind: "limiter", on: true, ...FX_DEFAULTS.limiter, gain: 0.15 }] },
    tracks: [
      { id: "t1", name: "Guitar", order: 1, owner: me, src: 0, arm: true, vol: 0.8, pan: 0, rack: [{ kind: "reverb" }], slot: 0, mine: true, mute: false, solo: false },
      { id: "t4", name: "Drums", order: 0.5, owner: me, src: 0, arm: false, vol: 0.8, pan: 0, rack: [], slot: 3, mine: true, mute: false, solo: false, inst: { kind: "drums", level: 0.8, tune: 0.5, decay: 0.5 } },
      { id: "t5", name: "Bass synth", order: 0.7, owner: partner, src: 0, arm: false, vol: 0.7, pan: 0, rack: [], slot: 4, mine: false, mute: false, solo: false, inst: { kind: "synth", wave: 0.3, cutoff: 0.4, resonance: 0.3, attack: 0.02, release: 0.2, detune: 0.2 } },
      { id: "t2", name: "Vocals", order: 2, owner: partner, src: 0, arm: true, vol: 0.75, pan: 0, rack: [], slot: 1, mine: false, mute: false, solo: false, sends: { b1: 0.6 } },
      { id: "t3", name: "Keys", order: 3, owner: partner, src: 1, arm: false, vol: 0.7, pan: 0, rack: [], slot: 2, mine: false, mute: false, solo: false, sends: { b1: 0.4 } },
    ],
    buses: [
      { id: "b1", name: "Reverb", order: 1001, owner: me, bus: true, vol: 0.8, pan: 0, rack: [{ kind: "reverb", size: 0.75, mix: 0.5, dry: 0 }], slot: 5, mine: true, mute: false, solo: false },
    ],
    clips: [
      { id: "c1", track: "t1", file: "take-g1.wav", start: 0, offset: 0, len: 15, gain: 1 },
      { id: "c2", track: "t2", file: "take-v1.wav", start: 8, offset: 0.5, len: 11, gain: 0.7, fade_in: 1.2, fade_out: 2.5 },
      { id: "c3", track: "t3", file: "take-k1.wav", start: 16, offset: 0, len: 8, gain: 1 },
      { id: "c4", track: "t4", pattern: "pa", start: 0, blen: 16, boff: 0 },
      { id: "c5", track: "t5", pattern: "pb", start: 8, blen: 16, boff: 0 },
    ],
    patterns: [{ id: "pa", name: "Beat", len: 4 }, { id: "pb", name: "Bassline", len: 8 }],
    notes: [
      ...[0, 1.5, 2, 3.5].map((b, i) => ({ id: "k" + i, pattern: "pa", track: "t4", start: b, len: 0.25, key: 36, vel: 115 })),
      ...[1, 3].map((b, i) => ({ id: "s" + i, pattern: "pa", track: "t4", start: b, len: 0.25, key: 38, vel: 105 })),
      ...Array.from({ length: 8 }, (_, i) => ({ id: "h" + i, pattern: "pa", track: "t4", start: i / 2, len: 0.25, key: 42, vel: i % 2 ? 70 : 100 })),
      ...[[0, 45, 1.5], [1.5, 45, 0.5], [2, 48, 1], [3, 50, 1], [4, 41, 1.5], [5.5, 41, 0.5], [6, 43, 1], [7, 48, 1]].map(([b, k, l], i) => ({ id: "b" + i, pattern: "pb", track: "t5", start: b, len: l, key: k, vel: 100 })),
    ],
    patternMode: null,
    files, me, partner, local: { click: true, countin: true }, conflict: null, shared: true, undo: true, redo: false,
  };
  let v = 1, playing = false, recording = false, recFrom = null, pos = 0, t0 = 0, exportAt = 0;
  const copy = () => JSON.parse(JSON.stringify(view));
  const now = () => (playing ? pos + (performance.now() - t0) / 1000 / (60 / view.meta.bpm) : pos);
  return {
    daw_project: async () => copy(),
    daw_edit: async (changes) => {
      for (const [kind, id, f] of changes) {
        if (kind === "meta") Object.assign(view.meta, f);
        const bus = kind === "track" && (f.bus || view.buses.some((b) => b.id === id));
        const list = { track: bus ? view.buses : view.tracks, clip: view.clips, pattern: view.patterns, note: view.notes }[kind];
        if (!list) continue;
        const o = list.find((x) => x.id === id);
        if (f.del) { if (o) list.splice(list.indexOf(o), 1); continue; }
        const used = new Set([...view.tracks, ...view.buses].map((t) => t.slot));
        let free = 0;
        while (used.has(free)) free++;
        if (o) Object.assign(o, f); else list.push({ id, ...f, mine: f.owner === me, slot: free });
      }
      v++; return copy();
    },
    daw_local: async (k, val, tid) => {
      if (tid) view.tracks.find((t) => t.id === tid)[k] = val; else view.local[k] = val;
      v++; return copy();
    },
    daw_play: async () => { pos = now(); playing = true; t0 = performance.now(); view.patternMode = null; },
    daw_play_pattern: async (pid) => { pos = 0; playing = true; t0 = performance.now(); view.patternMode = pid; v++; },
    daw_stop: async () => { playing = false; recording = false; pos = 0; view.patternMode = null; v++; },
    daw_record: async () => { playing = true; recording = true; recFrom = pos; t0 = performance.now(); return { ok: true }; },
    daw_locate: async (p) => { pos = p; t0 = performance.now(); },
    daw_peaks: async (n) => pk[n] || null,
    daw_projects: async () => [{ pid: "pdemo", name: view.meta.name, modified: Date.now() / 1000, clips: view.clips.length, current: true },
      { pid: "p2", name: "Blues in A", modified: Date.now() / 1000 - 86400, clips: 5, current: false }],
    daw_new: async () => copy(), daw_open: async () => copy(), daw_resolve: async () => copy(),
    daw_undo: async () => copy(), daw_redo: async () => copy(),
    daw_export: async (o) => { exportAt = performance.now(); return { ok: true, note: "", files: o.stems ? ["/Users/you/Music/Summer jam (stems)/Summer jam.wav", "a", "b", "c"] : ["/Users/you/Music/Summer jam.wav"] }; },
    daw_export_status: async () => {
      const p = Math.min(1, (performance.now() - exportAt) / 1500);
      return p < 1 ? { running: true, progress: p, result: null } : { running: false, progress: 1, result: { ok: true, warnings: [] } };
    },
    show_file: async () => {},
    daw_presence: async () => {},
    status: () => ({ v, pos: playing ? pos + (performance.now() - t0) / 1000 / (60 / view.meta.bpm) : pos, playing, recording, recFrom,
      levels: playing ? { t1: 0.4, t2: 0.25 } : {}, xfer: { "take-k1.wav": [3_100_000, 4_600_000, "in"] }, notices: [],
      ...demoPartner() }),
  };
}
