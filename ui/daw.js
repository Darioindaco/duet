"use strict";
// Duet: the song view. A playlist of tracks with recorded clips, a transport,
// and the song menu. The song itself lives in Python (daw.py), shared live with
// the partner; this page shows it and sends edits as [kind, id, {field: value}].
// Positions are in beats; clip offsets and lengths in seconds.
//
// Loaded before the main script; everything here runs once boot() calls
// songSetup(), so the main script's helpers ($, ui, api, toast…) exist by then.

const ROW = 64;          // track height, px
const TAKE_ROW = 34;     // a take's lane under an opened track, px
// ---------- the rows: a track is ROW high, plus a lane a take when its takes are open ----------
let _rows = null, _rowsKey = null;
function takeLanes(tid) {
  // lane j shows the j-th take of every take folder on the track
  const per = {};
  for (const c of (song.view && song.view.takes) || []) if (c.track === tid) (per[c.group] ||= new Set()).add(c.take);
  const groups = Object.entries(per).map(([g, s]) => [g, [...s].sort((a, b) => a - b)]);
  return { groups, n: Math.max(0, ...groups.map(([, s]) => s.length)) };
}
function rows() {
  const v = song.view;
  if (!v) return { y: [], h: [], lanes: [], total: 0 };
  const key = v.tracks.map((t) => t.id).join() + "|" + [...(song.takesOpen || [])].join() + "|" + (v.takes || []).length + "|" + (v.takes || []).map((c) => c.track + c.take).join();
  if (_rows && _rowsKey === key && _rows.v === v) return _rows;
  const y = [], h = [], lanes = [];
  let at = 0;
  v.tracks.forEach((t) => {
    const L = song.takesOpen && song.takesOpen.has(t.id) ? takeLanes(t.id) : { groups: [], n: 0 };
    y.push(at); lanes.push(L); h.push(ROW + L.n * TAKE_ROW);
    at += ROW + L.n * TAKE_ROW;
  });
  _rowsKey = key;
  return (_rows = { y, h, lanes, total: at, v });
}
const rowY = (i) => { const r = rows(); return i < r.y.length ? r.y[i] : r.total + (i - r.y.length) * ROW; };
const rowsTotal = () => rows().total;
// the track row at py (past the last: as if every row were ROW high), and the take lane in it (-1: the track itself)
function rowAt(py) {
  const r = rows();
  if (py >= r.total) return { row: r.y.length + Math.floor((py - r.total) / ROW), lane: -1 };
  let i = 0;
  while (i + 1 < r.y.length && r.y[i + 1] <= py) i++;
  const dy = py - (r.y[i] || 0);
  return { row: i, lane: dy < ROW ? -1 : Math.floor((dy - ROW) / TAKE_ROW) };
}
// -1 off (free), -2 smart (free, but a grid line within a few pixels catches it), else the grid in beats (0 = a bar)
const SNAPS = [["Off", -1], ["Smart", -2], ["Bar", 0], ["Beat", 1], ["1/2", 0.5], ["1/4", 0.25], ["1/8", 0.125], ["1/16", 0.0625]];
const song = {
  // tracks whose takes are open under them (this computer only)
  takesOpen: (() => { try { return new Set(JSON.parse(localStorage.getItem("duet.takesOpen") || "[]")); } catch (e) { return new Set(); } })(),
  view: null, v: -1, st: null, stAt: 0, fetching: false,
  peaks: {}, peakReq: {},
  sel: new Set(), selTrack: null,
  ppb: 22, snap: -1,         // pixels per beat, snap (beats; 0 = bar, -1 = off, -2 = smart)
  drag: null, rulerDrag: null, headBusy: false, pendingHeads: false,
  cv: null, ov: null, ruler: null, raf: 0, follow: true,
  colors: { me: "#ff9548", partner: "#4fd6c6" },
};

// ---------- small helpers ----------
const spb = () => 60 / (song.view ? song.view.meta.bpm : 120);
const bpb = () => (song.view ? song.view.meta.beats : 4);
// ---------- the tempo map: tempo and time signature changes (like daw.py's TempoMap) ----------
// meta.tempo [[beat, bpm]] after the start (meta.bpm), meta.meters [[beat, beats per bar]] (meta.beats)
let _tm = null, _tmKey = "";
function tmap() {
  const m = song.view ? song.view.meta : { bpm: 120, beats: 4 };
  const key = JSON.stringify([m.bpm, m.beats, m.tempo, m.meters]);
  if (_tm && key === _tmKey) return _tm;
  const ch = new Map([[0, Math.min(400, Math.max(20, +m.bpm || 120))]]);
  for (const x of m.tempo || []) if (Array.isArray(x) && +x[0] > 0 && +x[1] >= 20 && +x[1] <= 400) ch.set(+x[0], +x[1]);
  const beats = [...ch.keys()].sort((a, b) => a - b), bpms = beats.map((b) => ch.get(b)), secs = [0];
  for (let i = 1; i < beats.length; i++) secs.push(secs[i - 1] + (beats[i] - beats[i - 1]) * 60 / bpms[i - 1]);
  const mt = new Map([[0, Math.min(32, Math.max(1, Math.round(+m.beats || 4)))]]);
  for (const x of m.meters || []) if (Array.isArray(x) && +x[0] > 0 && +x[1] >= 1 && +x[1] <= 32) mt.set(+x[0], Math.round(+x[1]));
  const mbeats = [...mt.keys()].sort((a, b) => a - b), meters = mbeats.map((b) => mt.get(b));
  // the bar number each time signature starts at (a change off a barline starts a new bar)
  const mbar = [0];
  for (let i = 1; i < mbeats.length; i++) mbar.push(mbar[i - 1] + Math.ceil((mbeats[i] - mbeats[i - 1]) / meters[i - 1] - 1e-9));
  _tmKey = key;
  return (_tm = { beats, bpms, secs, mbeats, meters, mbar, steady: beats.length === 1 && mbeats.length === 1 });
}
const _seg = (xs, x) => { let i = 0; while (i + 1 < xs.length && xs[i + 1] <= x + 1e-9) i++; return i; };
function b2s(b) { const t = tmap(), i = _seg(t.beats, b); return t.secs[i] + (b - t.beats[i]) * 60 / t.bpms[i]; }
function s2b(s) { const t = tmap(), i = _seg(t.secs, s); return t.beats[i] + (s - t.secs[i]) * t.bpms[i] / 60; }
const bpmAt = (b) => { const t = tmap(); return t.bpms[_seg(t.beats, b)]; };
// beats per bar at beat b, and where that time signature starts
function meterAt(b) { const t = tmap(), i = _seg(t.mbeats, Math.max(0, b)); return { at: t.mbeats[i], n: t.meters[i], bar: t.mbar[i], i }; }
const bpbAt = (b) => meterAt(b).n;
// the barline at or before beat b, and the one after it
function barFloor(b) {
  const m = meterAt(b);
  return b < 0 ? Math.floor(b / m.n) * m.n : m.at + Math.floor((b - m.at) / m.n + 1e-9) * m.n;
}
function barNext(b) {
  const m = meterAt(b), t = tmap(), nx = barFloor(b) + m.n, ch = t.mbeats[m.i + 1];
  return ch !== undefined && ch < nx ? ch : nx;
}
// a bar number (0-based) and the beat in it
function barOf(b) {
  const m = meterAt(b), k = Math.floor((b - m.at) / m.n + 1e-9);
  return { bar: m.bar + k, beat: b - m.at - k * m.n, n: m.n };
}
// the beat a bar (0-based) starts at
function barBeat(bar) {
  const t = tmap();
  let i = 0;
  while (i + 1 < t.mbar.length && t.mbar[i + 1] <= bar) i++;
  return t.mbeats[i] + (bar - t.mbar[i]) * t.meters[i];
}
// seconds between two beats (a clip's length in seconds, a fade...) and back
const secsBetween = (a, b) => b2s(b) - b2s(a);
const beatsAfter = (a, secs) => s2b(b2s(a) + secs) - a;
const trackRow = (tid) => song.view.tracks.findIndex((t) => t.id === tid);
const tColor = (t) => (t && !t.mine ? song.colors.partner : song.colors.me);
// pattern clips are measured in beats (blen), audio clips in seconds (len)
const clipBeats = (c) => (c.pattern ? c.blen ?? 4 : beatsAfter(c.start || 0, c.len));
// the finest grid that's at least 12 px wide at this zoom (bar, beat, ... 1/16)
function zoomGrid() {
  for (const g of [0.0625, 0.125, 0.25, 0.5, 1]) if (g * song.ppb >= 12) return g;
  return bpb();
}
function snapBeat(b, mode = song.snap) {
  if (mode === -1) return b;
  if (mode === -2 || mode === -3) {
    // smart: only when a line of the grid you can see is within 6 px; -3: always to that grid
    const g = zoomGrid(), r = Math.round(b / g) * g;
    return mode === -3 || Math.abs(r - b) * song.ppb <= 6 ? r : b;
  }
  if (mode === 0) { const a = barFloor(b), z = barNext(b); return b - a < z - b ? a : z; }
  return Math.round(b / mode) * mode;
}
// while a clip is being split into stems the playlist is locked (moving things under the split would
// put the stems in the wrong place); everything else keeps working
const playlistLocked = () => !!(song.st && song.st.stems && song.st.stems.job);
function lockedToast() { toast("The playlist is locked while the stems are made — it's back in a moment (or Stop the split)."); }
// the snap for a mouse event: Alt turns it around (off → the grid you can see, on → free)
const snapFor = (ev) => (ev && ev.altKey ? (song.snap === -1 ? -3 : -1) : song.snap);
function fmtPos(beats) {
  if (beats < 0) {  // a count-in: in the first time signature
    const n = bpb(), b = -beats, bar = Math.floor(b / n), beat = Math.floor(b % n), tick = Math.floor((b % 1) * 100);
    return `-${bar + 1}.${beat + 1}.${String(tick).padStart(2, "0")}`;
  }
  const o = barOf(beats), tick = Math.floor((o.beat % 1) * 100);
  return `${o.bar + 1}.${Math.floor(o.beat + 1e-9) + 1}.${String(tick).padStart(2, "0")}`;
}
function fmtTime(beats) {
  const s = Math.max(0, b2s(beats));
  return `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, "0")}`;
}
function curPos() {
  const st = song.st;
  if (!st) return 0;
  if (!st.playing) return st.pos;
  return s2b(b2s(st.pos) + (performance.now() - song.stAt) / 1000);
}
function decodePeaks(b64) {
  const bin = atob(b64), out = new Int8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = (bin.charCodeAt(i) << 24) >> 24;
  return out;
}
// the same peaks at ever coarser steps (512, 1024, 2048... frames each), so a
// long file zoomed out draws from a few thousand values instead of millions
function peakLevels(pk) {
  const levels = [pk];
  while (pk.length > 4096) {
    const n = pk.length >> 2, next = new Int8Array(2 * n);
    for (let i = 0; i < n; i++) {
      next[2 * i] = Math.min(pk[4 * i], pk[4 * i + 2]);
      next[2 * i + 1] = Math.max(pk[4 * i + 1], pk[4 * i + 3]);
    }
    levels.push(next);
    pk = next;
  }
  return levels;
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
  // which sends there are (not how loud: a send knob being turned stays put)
  const routeSig = JSON.stringify([[...v.tracks, ...(v.buses || [])].map((t) => [t.id, Object.keys(t.sends || {}).sort()]),
    Object.entries(v.local?.livesends || {}).map(([k, s]) => [k, Object.keys(s).sort()])]);
  if (routeSig !== song.routeSig) { song.routeSig = routeSig; if (ui.mixer) refreshRouting(); }
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
  song.snap = store.get("songSnap", -1);
  if (!SNAPS.some(([, v]) => v === song.snap)) song.snap = -1;
  song.cv = $("lanesCv"); song.ov = $("lanesOv"); song.ruler = $("ruler");

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
  $("tgPunch").onclick = async () => setSongView(await api.daw_local("punch", !song.view.local.punch));
  $("tgLoop").onclick = () => {
    const m = song.view.meta;
    const ch = { loop: !m.loop };
    if (!m.loop && m.loop_end <= m.loop_start) Object.assign(ch, { loop_start: 0, loop_end: 4 * bpb() });
    songEdit([["meta", "meta", ch]]);
  };
  // snap: a small magnet button with a menu
  const paintSnap = () => {
    const label = (SNAPS.find(([, v]) => v === song.snap) || SNAPS[0])[0];
    $("snapLbl").textContent = label;
    $("snapBtn").classList.toggle("on", song.snap !== -1);
    $("snapBtn").title = song.snap === -1 ? "Snap: off, everything moves freely (hold Alt to snap to the grid you see)"
      : song.snap === -2 ? "Snap: smart, free, but a grid line close by catches it (hold Alt: free)" : `Snap to grid: ${label} (hold Alt to move freely)`;
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
  song.ruler.addEventListener("contextmenu", rulerContext);
  song.ruler.addEventListener("dblclick", rulerDbl);
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

// the views are windows now (wm.js): open the one for v and bring it to the front
function setMainView(v) {
  if (v === "seq") { wmOpen("rack", false); wmOpen("roll"); }
  else wmOpen(v === "mixer" ? "mixer" : "song");
}

// (the keys: keys.js)

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
  if (String(was && was.processing) !== String(d.processing)) { songDraw(); if (typeof ceDraw === "function") ceDraw(); }
  if (JSON.stringify(was && was.stems && was.stems.job) !== JSON.stringify(d.stems && d.stems.job)) { songDraw(); lockBanner(); }
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
  $("tgPunch").classList.toggle("on", !!v.local.punch);
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
  box.style.height = rowsTotal() + "px";
  v.tracks.forEach((t, i) => {
    const el = document.createElement("div");
    el.className = "th" + (t.mine ? "" : " partner") + (song.selTrack === t.id ? " sel" : "");
    el.dataset.id = t.id;
    el.style.top = rowY(i) + "px";
    el.style.height = rows().h[i] + "px";
    el.onpointerdown = () => { song.selTrack = t.id; for (const x of box.children) x.classList.toggle("sel", x === el); };

    const name = document.createElement("input");
    name.className = "tn"; name.value = t.name || "Track"; name.spellcheck = false; name.title = "Rename";
    name.onfocus = () => (song.headBusy = true);
    name.onblur = () => { song.headBusy = false; if (name.value.trim() && name.value !== t.name) songEdit([["track", t.id, { name: name.value.trim() }]]); else if (song.pendingHeads) renderHeads(); };
    name.onkeydown = (e) => { if (e.key === "Enter") name.blur(); if (e.key === "Escape") { name.value = t.name; name.blur(); } };
    if (isAuto(t)) {
      el.append(name);
      autoHead(t, el);
      box.append(el);
      return;
    }
    el.oncontextmenu = (e) => { if (e.target.closest("input, select")) return; e.preventDefault(); autoMenu(t.id, e.clientX, e.clientY); };

    const btns = document.createElement("div");
    btns.className = "tbtns";
    const arm = document.createElement("button");
    arm.className = "arm" + (t.arm ? " on" : "");
    arm.title = t.inst ? "Record the notes you play on this channel" : t.mine ? "Record on this track (from your input)" : "Record on this track (from your partner's input, on their side)";
    arm.onclick = () => songEdit([["track", t.id, { arm: !t.arm }]]);
    const del = document.createElement("button");
    del.className = "del"; del.textContent = "✕"; del.title = "Delete this track";
    del.onclick = () => deleteTrack(t);
    if ((v.takes || []).some((c) => c.track === t.id)) {
      const open = song.takesOpen && song.takesOpen.has(t.id);
      const tk = document.createElement("button");
      tk.className = "tb takesbtn" + (open ? " on" : "");
      tk.textContent = open ? "▾" : "▸";
      tk.title = open ? "Close the takes" : "Show the takes under the track (comp them here)";
      tk.onclick = () => toggleTakes(t.id);
      btns.append(tk);
    }
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
  // a channel's notes go with it (in every pattern), and so do its takes
  const notes = (song.view.notes || []).filter((n) => n.track === t.id);
  const takes = (song.view.takes || []).filter((c) => c.track === t.id);
  songEdit([["track", t.id, { del: true }], ...[...clips, ...takes].map((c) => ["clip", c.id, { del: true }]),
    ...notes.map((n) => ["note", n.id, { del: true }])]);
}

function trackBySlot(slot) {
  return song.view ? [...song.view.tracks, ...(song.view.buses || [])].find((t) => t.slot === slot) : null;
}

function openTrackFx(t) {
  if (t.slot == null) { toast("This song has more tracks than the mixer can hold (32)."); return; }
  setMainView("mixer");
  selectStrip("T" + t.slot);
  const el = document.querySelector(`#strips .strip[data-t="T${t.slot}"]`);
  el?.scrollIntoView({ inline: "center", block: "nearest" });
  // a channel's "Sound": its instrument's window
  if (t.inst && el) openFx("T" + t.slot, "inst", el.querySelector(".slot.inst"));
}

// ---------- drawing ----------
// Sizes a canvas's pixels to w×h CSS pixels (its client size). With `css` it also fixes its CSS size;
// leave that off for a canvas whose size comes from the stylesheet (width: 100%), or it can't follow the
// window any more. (The CSS size includes the border: with box-sizing: border-box, setting the client size
// as the width would take the border off it at every redraw, and the canvas would shrink 2 px a frame.)
function sizeCanvas(cv, w, h, css = true) {
  const dpr = window.devicePixelRatio || 1;
  if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) {
    cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr);
    if (css) {
      const bx = cv.offsetWidth - cv.clientWidth, by = cv.offsetHeight - cv.clientHeight;
      cv.style.width = w + bx + "px"; cv.style.height = h + by + "px";
    }
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

// every bar from beat a to b: fn(bar number, the beat it starts at, its length in beats, bar number)
function eachBar(a, b, fn) {
  let k = barOf(Math.max(0, a)).bar;
  for (let guard = 0; guard < 4000; guard++, k++) {
    const at = barBeat(k);
    if (at > b) break;
    const len = barBeat(k + 1) - at;
    fn(k, at, len, k);
  }
}

function gridStep(n = bpb()) {
  // the finest grid that stays readable
  if (song.ppb >= 48) return 0.25;
  if (song.ppb >= 12) return 1;
  if (song.ppb * n >= 24) return n;
  return n * 4;
}

function songDraw() {
  if (!song.view || !winShown("song")) return;
  const lanes = $("lanes");
  const w = lanes.clientWidth, h = lanes.clientHeight;
  if (!w || !h) return;
  const v = song.view, ppb = song.ppb, sx = lanes.scrollLeft, sy = lanes.scrollTop;
  $("lanesSize").style.width = songLengthBeats() * ppb + "px";
  $("lanesSize").style.height = Math.max(h, rowsTotal() + 2 * ROW) + "px";
  const ctx = sizeCanvas(song.cv, w, h);
  ctx.clearRect(0, 0, w, h);
  // rows
  v.tracks.forEach((t, i) => {
    const y = rowY(i) - sy, rh = rows().h[i];
    if (y > h || y + rh < 0) return;
    ctx.fillStyle = i % 2 ? "#111017" : "#14121a";
    ctx.fillRect(0, y, w, rh);
    if (rh > ROW) { ctx.fillStyle = "#0b0a0f"; ctx.fillRect(0, y + ROW, w, rh - ROW); }
    ctx.fillStyle = "#1f1c27";
    ctx.fillRect(0, y + rh - 1, w, 1);
  });
  // grid
  const rowsH = Math.min(h, rowsTotal() - sy);
  eachBar(sx / ppb, (sx + w) / ppb, (bar, at, len, k) => {
    const step = gridStep(len);
    if (step > len && k % Math.round(step / len)) return; // zoomed far out: every 4th bar
    for (let b = 0; b < Math.min(len, step > len ? len : 1e9) - 1e-9; b += Math.min(step, len)) {
      const x = Math.round((at + b) * ppb - sx) + 0.5;
      const beat = at + b;
      ctx.fillStyle = b === 0 ? "rgba(255,255,255,0.10)" : Math.abs(b - Math.round(b)) < 1e-6 ? "rgba(255,255,255,0.045)" : "rgba(255,255,255,0.02)";
      ctx.fillRect(x - 0.5, 0, 1, Math.max(0, rowsH));
      if (beat > (sx + w) / ppb) break;
    }
  });
  // loop
  if (v.meta.loop) {
    const x0 = v.meta.loop_start * ppb - sx, x1 = v.meta.loop_end * ppb - sx;
    ctx.fillStyle = "rgba(79,214,198,0.05)";
    ctx.fillRect(x0, 0, x1 - x0, Math.max(0, rowsH));
  }
  // sections: a line where each starts, the selected one lit
  for (const sc of sections()) {
    const x0 = sc.a * ppb - sx, x1 = sc.b * ppb - sx;
    if (x1 < 0 || x0 > w) continue;
    if (song.selSection && song.selSection.id === sc.m.id) { ctx.fillStyle = sc.color + "14"; ctx.fillRect(x0, 0, x1 - x0, Math.max(0, rowsH)); }
    ctx.fillStyle = sc.color + "66";
    ctx.fillRect(Math.round(x0), 0, 1, Math.max(0, rowsH));
  }
  // clips (while dragging: where they'd end up; a copy-drag leaves the originals)
  const d = song.drag, res = d && d.moved ? d.result || {} : {};
  const place = (c, ghost) => {
    const row = ghost && ghost.row != null ? ghost.row : trackRow(c.track);
    if (row < 0) return;
    const cc = ghost ? { ...c, ...ghost } : c;
    const x = cc.start * ppb - sx, cw = clipBeats(cc) * ppb, y = rowY(row) - sy;
    if (x > w || x + cw < 0 || y > h || y + ROW < 0) return;
    drawClip(ctx, cc, x, y + 2, cw, ROW - 5, v.tracks[row]);
  };
  v.tracks.forEach((t, i) => {
    const y = rowY(i) - sy;
    if (isAuto(t) && y < h && y + ROW > 0) drawAutoLane(ctx, t, y, sx, w);
    if (rows().lanes[i] && rows().lanes[i].n) drawTakeLanes(ctx, t, i, y, sx, w, h);
  });
  for (const c of v.clips) place(c, d && !d.copy ? res[c.id] : null);
  if (d && d.copy) for (const c of v.clips) if (res[c.id]) place(c, res[c.id]);
  drawRuler();
  drawOverlay();
}

// what's been done to a clip, after its name: ◀ backwards, ±semitones, the speed
function clipBadges(c) {
  let out = "";
  if (c.group) out += `  ⧉ take ${c.take}`;
  if (c.tune && (c.tune.notes || []).length) out += "  ♪ tuned";
  if (c.reverse) out += "  ◀";
  const p = c.pitch || 0;
  if (Math.abs(p) >= 0.005) out += `  ${p > 0 ? "+" : ""}${Number.isInteger(+p.toFixed(2)) ? p.toFixed(0) : p.toFixed(2)} st`;
  const sp = c.speed ?? 1;
  if (Math.abs(sp - 1) >= 0.0005) out += `  ${Math.round(sp * 1000) / 10}%`;
  return out;
}
const stretching = (c) => !!(((c.stretch && (Math.abs((c.speed ?? 1) - 1) > 1e-4 || Math.abs(c.pitch || 0) > 0.005))
  || (c.tune && (c.tune.notes || []).length)) && (song.st?.processing || []).includes(c.file));

function drawClip(ctx, c, x, y, w, h, track) {
  if (c.pattern) { drawPatternClip(ctx, c, x, y, w, h); return; }
  const col = tColor(track), sel = song.sel.has(c.id);
  const r = 6;
  ctx.save();
  if (c.mute || c._erasing) ctx.globalAlpha = 0.35; // muted (or being erased): still there, greyed
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
    const levels = song.peaks[c.file];
    if (levels) {
      const sr = file.sr || 48000, secPerPx = 60 / bpmAt(c.start) / song.ppb, s0 = b2s(c.start);
      const tAt = (px) => b2s(c.start + px / song.ppb) - s0; // seconds into the clip at this pixel
      let k = 0;
      while (k + 1 < levels.length && (512 << (k + 1)) <= secPerPx * sr * clipRatio(c)) k++;
      const pk = levels[k], per = 512 << k;
      const lv = document.querySelector("#lanes").clientWidth;
      const px0 = Math.max(0, Math.floor(-x)), px1 = Math.min(Math.ceil(w), Math.ceil(lv - x));
      ctx.fillStyle = col + "e0";
      const g = (c.gain ?? 1) * (wh / 2) / 127;
      const r = clipRatio(c);
      for (let px = px0; px < px1; px++) {
        // the stretch of the file this pixel plays (backwards: read from the end)
        const ta = tAt(px), tb = tAt(px + 1);
        const a = clipSrcAt(c, ta), b = clipSrcAt(c, tb);
        const t0 = Math.min(a, b), w = (tb - ta) * r;
        let i0 = Math.floor(t0 * sr / per), i1 = Math.max(i0 + 1, Math.floor((t0 + w) * sr / per));
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
      api.daw_peaks(c.file).then((b) => { if (b) { song.peaks[c.file] = peakLevels(decodePeaks(b)); songDraw(); } else song.peakReq[c.file] = false; });
    }
  } else {
    // audio still on its way from the partner
    ctx.strokeStyle = col + "40";
    ctx.lineWidth = 6;
    for (let k = -wh; k < w; k += 16) { ctx.beginPath(); ctx.moveTo(x + k, wy + wh); ctx.lineTo(x + k + wh, wy); ctx.stroke(); }
    const xf = (song.st && song.st.xfer || {})[c.file];
    const label = file && file.loading ? "Loading…" : xf && xf[1] ? `Receiving ${Math.round(xf[0] / xf[1] * 100)}%`
      : song.view.shared ? "Waiting for the audio…" : "Audio is with your partner";
    ctx.fillStyle = "#cfc8da";
    ctx.font = "600 11px system-ui, sans-serif";
    ctx.fillText(label, Math.max(x, 0) + 8, mid + 4);
  }
  // fades: shade what's faded out, a line where the level goes
  const fin = fadePx(c, "in", song.ppb), fout = fadePx(c, "out", song.ppb);
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
  const label = (track ? track.name : "Clip") + (c.mute ? "  (muted)" : Math.abs(g - 1) > 0.001 ? `  ${fmtDb(g)}` : "") + clipBadges(c);
  ctx.fillText(label, Math.max(x, 0) + 6, y + 11);
  // being split into stems
  const sj = song.st && song.st.stems && song.st.stems.job;
  if (sj && sj.clip === c.id) {
    ctx.fillStyle = "rgba(8,7,11,0.6)";
    ctx.fillRect(Math.max(x, 0) + 6, mid - 9, 132, 18);
    ctx.fillStyle = "#9fe8dc";
    ctx.font = "600 11px system-ui, sans-serif";
    ctx.fillText(sj.stage === "splitting" ? `Splitting… ${Math.round(sj.progress * 100)}%` : "Splitting into stems…", Math.max(x, 0) + 12, mid + 4);
  }
  // a keep-pitch clip whose stretched copy is still being made: silent till then
  if (stretching(c)) {
    ctx.fillStyle = "rgba(8,7,11,0.6)";
    ctx.fillRect(Math.max(x, 0) + 6, mid - 9, 92, 18);
    ctx.fillStyle = "#ffe2a3";
    ctx.font = "600 11px system-ui, sans-serif";
    ctx.fillText(c.tune && (c.tune.notes || []).length && !c.stretch ? "Tuning…" : "Stretching…", Math.max(x, 0) + 12, mid + 4);
  }
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
  ctx.globalAlpha = c.mute || c._erasing ? 0.35 : 1; // muted: greyed
  if (p) {
    const notes = (song.view.notes || []).filter((n) => n.pattern === p.id && n.track === c.track); // its own channel's
    if (notes.length) {
      // a looping block repeats one round of the pattern; a "once" block plays it through once
      const plen = c.once ? 1e9 : patRound(p), boff = c.boff || 0;
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

// the ruler: markers and sections on top, then tempo and time signature changes, then bars and the loop
const RULER_H = 52, RM = 14, RT = 26; // (markers lane 0..RM, tempo lane RM..RT, bars below RT)
const MARK_COLORS = ["#ff9548", "#4fd6c6", "#b48cff", "#ffd166", "#ff6b8b", "#6bb6ff", "#8fdc6a"];
const markers = () => (song.view && song.view.markers) || [];
const markColor = (m, i) => m.color || MARK_COLORS[i % MARK_COLORS.length];
// the end of the song (for the last section): the end of the last clip, on a barline
function songEnd() {
  let end = 0;
  for (const c of song.view.clips) end = Math.max(end, c.start + clipBeats(c));
  for (const m of markers()) end = Math.max(end, m.beat + bpbAt(m.beat));
  return barFloor(end - 1e-6) < end - 1e-6 ? barNext(end - 1e-6) : end;
}
// sections: each section marker until the next one (the last one until the song's end)
function sections() {
  const ms = markers().filter((m) => m.section).sort((a, b) => a.beat - b.beat), end = songEnd();
  return ms.map((m, i) => ({ m, a: m.beat, b: i + 1 < ms.length ? ms[i + 1].beat : Math.max(end, m.beat + bpbAt(m.beat)),
    color: markColor(m, markers().indexOf(m)) }));
}
// the tempo and time signature changes, with the start's
function tempoTags() {
  const m = song.view.meta, out = [{ kind: "tempo", beat: 0, v: m.bpm }, { kind: "meter", beat: 0, v: m.beats }];
  for (const [b, x] of m.tempo || []) out.push({ kind: "tempo", beat: +b, v: +x });
  for (const [b, x] of m.meters || []) out.push({ kind: "meter", beat: +b, v: +x });
  return out;
}
const tagText = (t) => (t.kind === "tempo" ? `♩ ${+(+t.v).toFixed(2)}` : `${t.v}/4`);

function drawRuler() {
  const cv = song.ruler, w = $("lanewrap").clientWidth, h = RULER_H;
  const ctx = sizeCanvas(cv, w, h);
  const v = song.view, ppb = song.ppb, sx = $("lanes").scrollLeft;
  ctx.fillStyle = "#16141c";
  ctx.fillRect(0, 0, w, h);
  // sections and markers (and where each is, for the mouse)
  ctx.font = "600 10px system-ui, sans-serif";
  const sel = song.selSection, hits = (song.rulerHits = []);
  for (const sc of sections()) {
    const x0 = sc.a * ppb - sx, x1 = sc.b * ppb - sx;
    if (x1 < 0 || x0 > w) continue;
    const on = sel && sel.id === sc.m.id;
    ctx.fillStyle = sc.color + (on ? "70" : "38");
    ctx.fillRect(x0, 1, x1 - x0 - 1, RM - 2);
    ctx.fillStyle = sc.color;
    ctx.fillRect(x0, 1, 2, RM - 2);
    ctx.save(); ctx.beginPath(); ctx.rect(x0, 0, Math.max(0, x1 - x0 - 3), RM); ctx.clip();
    ctx.fillStyle = on ? "#fff" : "#e6e0ee";
    ctx.fillText(sc.m.name || "Section", Math.max(x0, 0) + 5, RM - 4);
    ctx.restore();
    hits.push({ what: "section", x0, x1, sc, m: sc.m });
  }
  markers().forEach((m, i) => {
    if (m.section) return;
    const x = m.beat * ppb - sx;
    if (x < -200 || x > w) return;
    ctx.fillStyle = markColor(m, i);
    ctx.beginPath(); ctx.moveTo(x, 1); ctx.lineTo(x + 7, 1); ctx.lineTo(x + 7, RM - 4); ctx.lineTo(x, RM - 1); ctx.fill();
    ctx.fillStyle = "#d8d2e2";
    ctx.fillText(m.name || "Marker", x + 10, RM - 4);
    hits.push({ what: "marker", x0: x - 2, x1: x + 12 + ctx.measureText(m.name || "Marker").width, m });
  });
  // tempo and time signature changes
  ctx.fillStyle = "#1c1a23"; ctx.fillRect(0, RM, w, RT - RM);
  ctx.font = "600 9.5px system-ui, sans-serif";
  const tags = tempoTags().sort((a, b) => a.beat - b.beat || (a.kind === "meter" ? -1 : 1));
  let lastX = -1e9;
  for (const t of tags) {
    let x = t.beat * ppb - sx;
    if (x > w) continue;
    const tw = ctx.measureText(tagText(t)).width + 8;
    const drawX = Math.max(x, lastX + 2);  // a meter and a tempo on one beat: side by side
    lastX = drawX + tw;
    if (lastX < 0) continue;
    const dr = song.tagDrag && song.tagDrag.kind === t.kind && song.tagDrag.beat === t.beat;
    ctx.fillStyle = t.kind === "tempo" ? (dr ? "#ffb070" : "#3a2c22") : (dr ? "#9fe8de" : "#203330");
    ctx.fillRect(drawX, RM + 1, tw, RT - RM - 2);
    ctx.fillStyle = t.kind === "tempo" ? "#ffb070" : "#7fe0d4";
    ctx.fillRect(x, RM + 1, 1, RT - RM - 2);
    ctx.fillStyle = dr ? "#16141c" : t.kind === "tempo" ? "#ffcf9f" : "#aef0e7";
    ctx.fillText(tagText(t), drawX + 4, RT - 3);
    hits.push({ what: "tag", x0: drawX, x1: drawX + tw, t });
  }
  ctx.save();
  ctx.translate(0, RT);
  ctx.beginPath(); ctx.rect(0, 0, w, h - RT); ctx.clip();
  const punch = v.local.punch && v.meta.loop_end > v.meta.loop_start;
  if (v.meta.loop || song.rulerDrag || punch) {
    const x0 = v.meta.loop_start * ppb - sx, x1 = v.meta.loop_end * ppb - sx;
    // punch on: the in and out points in red (recording keeps only what's between them)
    ctx.fillStyle = punch && !song.rulerDrag ? "rgba(255,96,112,0.7)" : v.meta.loop ? "rgba(79,214,198,0.55)" : "rgba(255,255,255,0.14)";
    ctx.fillRect(x0, 0, x1 - x0, 8);
  }
  const barPx = ppb * bpb();
  const every = barPx >= 36 ? 1 : barPx >= 14 ? 4 : 8;
  ctx.font = "600 10.5px system-ui, sans-serif";
  eachBar(sx / ppb, (sx + w) / ppb, (bar, at, len) => {
    const x = Math.round(at * ppb - sx) + 0.5;
    if (bar % every === 0) {
      ctx.fillStyle = "#5f586b"; ctx.fillRect(x - 0.5, 10, 1, 16);
      ctx.fillStyle = "#8e879b"; ctx.fillText(String(bar + 1), x + 4, 21);
    } else if (every === 1 || bar % (every / 2) === 0) {
      ctx.fillStyle = "#2f2b38"; ctx.fillRect(x - 0.5, 18, 1, 8);
    }
    if (len * ppb >= 60) for (let k = 1; k < len; k++) { ctx.fillStyle = "#2a2634"; ctx.fillRect(Math.round(x + k * ppb), 21, 1, 5); }
  });
  // playhead marker
  const px = curPos() * ppb - sx;
  ctx.fillStyle = song.st && song.st.recording ? "#ff6070" : "#efeaf4";
  ctx.beginPath(); ctx.moveTo(px - 5, 12); ctx.lineTo(px + 5, 12); ctx.lineTo(px, 20); ctx.fill();
  hereDrawRuler(ctx, sx);
  ctx.restore();
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
      const y = rowY(i) - sy + 2;
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
  ctx.fillRect(px - 0.5, 0, 1, Math.min(h, rowsTotal() - sy));
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
  // a sound being dragged in from the browser: where it would land
  const g = song.dropGhost;
  if (g) {
    const x = g.beat * ppb - sx, y = rowY(g.row) - sy + 2;
    ctx.fillStyle = "rgba(255,149,72,0.18)";
    ctx.fillRect(x, y, Math.max(40, 4 * ppb), ROW - 5);
    ctx.fillStyle = song.colors.me;
    ctx.fillRect(Math.round(x), y, 2, ROW - 5);
  }
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
  if (!winShown("song")) return;
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
  return { inside: x >= 0 && y >= 0 && x <= lanes.clientWidth && y <= lanes.clientHeight, px, py, beat: px / song.ppb, ...rowAt(py) };
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

// how wide a clip's fade is on screen (fades are in seconds; the tempo can change under them)
function fadePx(c, which, ppb) {
  if (which === "in") return beatsAfter(c.start, c.fade_in || 0) * ppb;
  const end = c.start + clipBeats(c);
  return (end - s2b(b2s(end) - (c.fade_out || 0))) * ppb;
}

function hitZone(p) {
  const c = hitClip(p.beat, p.row);
  if (!c) return null;
  const ppb = song.ppb, w = clipBeats(c) * ppb, lx = p.px - c.start * ppb, ly = p.py - (rowY(p.row) + 2);
  const fin = fadePx(c, "in", ppb), fout = fadePx(c, "out", ppb);
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
      gain: c.gain ?? 1, fade_in: c.fade_in || 0, fade_out: c.fade_out || 0,
      speed: c.speed ?? 1, pitch: c.pitch || 0, stretch: !!c.stretch, reverse: !!c.reverse });
// file seconds per timeline second (in tape mode the pitch speeds it up too)
const clipRatio = (c) => (c.speed ?? 1) * (c.stretch ? 1 : 2 ** ((c.pitch || 0) / 12));
// where in the file the clip is at timeline second t (from its start)
const clipSrcAt = (c, t) => (c.offset || 0) + (c.reverse ? c.len - t : t) * clipRatio(c);
// a cut at timeline second t: the file offsets of the part before and after it
const cutOffsets = (c, t) => {
  const r = clipRatio(c), o = c.offset || 0;
  return c.reverse ? { left: o + (c.len - t) * r, right: o } : { left: o, right: o + t * r };
};
const fileDur = (c) => { const f = song.view.files[c.file]; return f && f.frames ? f.frames / f.sr : Infinity; };
const gridOf = (mode) => (mode === -1 ? 0 : mode < 0 ? zoomGrid() : mode === 0 ? bpb() : mode);
const selClips = () => song.view.clips.filter((c) => song.sel.has(c.id));
const MIN_LEN = 0.05; // seconds

function lanesHover(e) {
  if (song.drag || song.band || song.autoDrag || e.buttons || !song.view) return;
  const p = lanePoint(e), at = p.inside ? song.view.tracks[p.row] : null;
  if (isAuto(at) || song.autoHover) { autoHoverAt(isAuto(at) ? at : null, p); if (isAuto(at)) return; }
  const h = p.inside && p.lane < 0 ? hitZone(p) : null;
  const tool = song.tool && song.tool !== "pointer" && !(song.tool === "pencil" && h);
  // Loop on: a clip's end drags out more rounds of it (the cursor says so)
  const loopEnd = h && h.zone === "right" && song.patLoop && (h.c.pattern || canLoopAudio(h.c));
  $("lanes").style.cursor = tool ? toolCursor() : loopEnd ? LOOP_CURSOR : h ? CURSORS[h.zone] : p.inside && p.lane >= 0 && takeLaneAt(p) ? "crosshair" : "";
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
  const row = song.view.tracks[p.row];
  if (playlistLocked()) { lanes.releasePointerCapture?.(e.pointerId); lockedToast(); return; }
  if (isAuto(row)) { autoDown(e, p, row); return; }
  if (p.lane >= 0) { takeLaneDown(e, p); return; }
  if (song.tool && song.tool !== "pointer" && toolDown(e, p)) return;
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
    d.result = dragResult(d, lanePoint(ev), snapFor(ev));
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
  const v = song.view, c = d.anchor, out = {}, cs = b2s(c.start), ce = b2s(c.start + clipBeats(c));
  const clampRow = (r, from) => { const k = Math.max(0, Math.min(v.tracks.length - 1, r)); return isAuto(v.tracks[k]) ? from : k; };
  if (d.zone === "body") {
    const db = Math.max(0, snapBeat(q.beat - d.grab, snap)) - c.start;
    const minStart = Math.min(...d.sel.map((k) => k.start));
    const shift = Math.max(db, -minStart);
    const dr = q.row - d.row0;
    for (const k of d.sel) out[k.id] = { start: k.start + shift, row: clampRow(trackRow(k.track) + dr, trackRow(k.track)) };
  } else if (c.pattern && d.zone === "left") {
    // a pattern clip can start anywhere in its pattern (and wraps round)
    const hi = c.start + clipBeats(c) - STEP;
    const start = Math.min(hi, Math.max(0, snapBeat(q.beat, snap)));
    out[c.id] = { start, boff: Math.max(0, (c.boff || 0) + start - c.start), blen: c.blen - (start - c.start) };
  } else if (c.pattern && d.zone === "right") {
    const end = Math.max(c.start + STEP, snapBeat(q.beat, snap));
    // Loop on: more rounds of it; off: the pattern once, as long as the block
    out[c.id] = { blen: end - c.start, once: !song.patLoop };
  } else if (d.zone === "left") {
    // how much file there is before the clip's start (backwards: after its end)
    const r = clipRatio(c), o = c.offset || 0;
    const room = c.reverse ? fileDur(c) - o - c.len * r : o;
    const lo = Math.max(0, s2b(cs - room / r)), hi = s2b(ce - MIN_LEN);
    const start = Math.min(hi, Math.max(lo, snapBeat(q.beat, snap)));
    const dt = b2s(start) - cs, len = c.len - dt;
    out[c.id] = { start, offset: c.reverse ? o : o + dt * r, len, fade_in: Math.min(c.fade_in || 0, len - (c.fade_out || 0)) };
  } else if (d.zone === "right") {
    const r = clipRatio(c), o = c.offset || 0;
    const room = c.reverse ? o + c.len * r : fileDur(c) - o; // file seconds the clip can reach
    const lo = s2b(cs + MIN_LEN), hi = s2b(cs + room / r);
    const end = Math.min(hi, Math.max(lo, snapBeat(q.beat, snap)));
    const len = b2s(end) - cs;
    out[c.id] = { len, fade_out: Math.min(c.fade_out || 0, len - (c.fade_in || 0)) };
    if (c.reverse) out[c.id].offset = Math.max(0, o - (len - c.len) * r);
  } else if (d.zone === "fin") {
    out[c.id] = { fade_in: Math.max(0, Math.min(c.len - (c.fade_out || 0), b2s(q.beat) - cs)) };
  } else if (d.zone === "fout") {
    out[c.id] = { fade_out: Math.max(0, Math.min(c.len - (c.fade_in || 0), ce - b2s(q.beat))) };
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
    const moved = new Set();
    // looping off: a pattern block dragged longer makes the pattern longer (not another round of it)
    const pc = d.anchor;
    // (and shorter again, but never shorter than one round of it): the block plays the pattern once,
    // as long as it is; one round (what loops) stays what it was
    if (!song.patLoop && d.zone === "right" && pc.pattern && d.result[pc.id]) {
      const p = (v.patterns || []).find((q) => q.id === pc.pattern);
      if (p) {
        const round = patRound(p), len = Math.max(round, (pc.boff || 0) + d.result[pc.id].blen);
        const f = {};
        if (p.base == null) f.base = round;
        if (Math.abs(len - patLen(p)) > 1e-6) f.len = len;
        if (Object.keys(f).length) ch.push(["pattern", p.id, f]);
      }
    }
    for (const [id, r] of Object.entries(d.result)) {
      const { row, ...f } = r;
      if (row != null) f.track = v.tracks[row].id;
      ch.push(["clip", id, f]);
      moved.add(id);
      // a clip of a take folder moves the whole folder with it (its takes and the rest of the comp)
      const c = v.clips.find((k) => k.id === id);
      if (d.zone === "body" && c && c.group) {
        const db = f.start - c.start;
        for (const k of [...v.clips, ...(v.takes || [])]) {
          if (k.group !== c.group || moved.has(k.id) || d.result[k.id]) continue;
          moved.add(k.id);
          ch.push(["clip", k.id, { start: k.start + db, ...(f.track ? { track: f.track } : {}) }]);
        }
      }
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
      if (rowY(row) + ROW > ly && rowY(row) < hy && c.start + clipBeats(c) > lb && c.start < hb) song.sel.add(c.id);
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
      const at = Math.max(0, snapBeat(p.beat, snapFor(e)));
      api.daw_locate(at);
      if (song.st && !song.st.playing) song.st.pos = at;
    }
    songDraw();
  };
  lanes.addEventListener("pointermove", move);
  lanes.addEventListener("pointerup", up);
}

function lanesDbl(e) {
  // double-click: a pattern opens in the sequencer, an audio clip in the
  // clip editor; empty space gets the current pattern
  if (playlistLocked()) { lockedToast(); return; }
  const p = lanePoint(e), c = hitClip(p.beat, p.row);
  if (isAuto(song.view.tracks[p.row])) return;
  if (!c) { insertPattern(p); return; }
  if (c.pattern) { seq.pat = c.pattern; setMainView("seq"); return; }
  openClipEditor(c.id);
}

function muteSel(on) {
  const sel = selClips();
  if (sel.length) songEdit(sel.map((k) => ["clip", k.id, { mute: on ?? !sel[0].mute }]));
}

// every clip on the track from this one on (it used to be the double-click)
function selectFrom(c) {
  for (const k of song.view.clips) if (k.track === c.track && k.start >= c.start) song.sel.add(k.id);
  songDraw();
}

function insertPattern(p) {
  const pat = curPattern(), t = song.view.tracks[p.row];
  if (!pat || !t || isAuto(t)) return;
  const at = Math.max(0, barFloor(p.beat));
  const id = newId("c");
  song.sel = new Set([id]);
  songEdit([["clip", id, { track: t.id, pattern: pat.id, start: at, blen: patLen(pat), boff: 0 }]]);
}

// ---------- editing: split, duplicate, copy/paste, delete ----------
function splitClips(beat, clips) {
  const ch = [];
  for (const c of clips) {
    if (beat <= c.start + 1e-6 || beat >= c.start + clipBeats(c) - 1e-6) continue;
    if (c.pattern) {
      const lb = beat - c.start;
      ch.push(["clip", c.id, { blen: lb }]);
      ch.push(["clip", newId("c"), { ...clipFields(c), start: beat, boff: (c.boff || 0) + lb, blen: c.blen - lb }]);
      continue;
    }
    const left = secsBetween(c.start, beat), right = c.len - left, cut = cutOffsets(c, left);
    ch.push(["clip", c.id, { offset: cut.left, len: left, fade_out: 0, fade_in: Math.min(c.fade_in || 0, left) }]);
    ch.push(["clip", newId("c"), { ...clipFields(c), ...(c.group ? { group: c.group, take: c.take } : {}), start: beat, offset: cut.right, len: right,
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
  const ch = ids.map((id) => ["clip", id, { del: true }]);
  // a take folder with none of its comp left: its takes go too
  const v = song.view, gone = new Set(ids);
  for (const g of new Set(v.clips.filter((c) => gone.has(c.id) && c.group).map((c) => c.group))) {
    if (v.clips.some((c) => c.group === g && !gone.has(c.id))) continue;
    for (const t of v.takes || []) if (t.group === g) ch.push(["clip", t.id, { del: true }]);
  }
  if (ch.length) songEdit(ch);
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
    if (!track || !v.tracks.some((t) => t.id === track && !isAuto(t))) continue;
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
  const row = song.view.tracks[p.row];
  if (playlistLocked()) { lockedToast(); return; }
  if (isAuto(row)) { autoContext(p, row, e.clientX, e.clientY); return; }
  if (p.lane >= 0) { takeLaneContext(e, p); return; }
  const h = hitZone(p), items = [];
  if (h) {
    const c = h.c;
    if (!song.sel.has(c.id)) { song.sel.clear(); song.sel.add(c.id); songDraw(); }
    const at = snapBeat(p.beat, snapFor(e));
    const n = song.sel.size;
    items.push(["Split here", "S", () => splitClips(at > c.start && at < c.start + clipBeats(c) ? at : p.beat, selClips())]);
    items.push([n > 1 ? `Duplicate ${n} clips` : "Duplicate", "Ctrl+D", duplicateSel]);
    items.push(["Copy", "Ctrl+C", () => copySel()]);
    items.push([n > 1 ? `Delete ${n} clips` : "Delete", "Del", deleteSel]);
    if (c.pattern) {
      items.push(["Open in the sequencer", "dbl-click", () => { seq.pat = c.pattern; setMainView("seq"); }]);
    } else {
      items.push(["Edit clip…", "dbl-click", () => openClipEditor(c.id)]);
      items.push(...takeMenuItems(c));
      items.push(["Split into stems…", "", () => { openClipEditor(c.id); ce.tab = "stems"; ceRender(); }]);
      items.push([c.mute ? "Unmute clip" : "Mute clip", "Alt+M", () => muteSel(!c.mute)]);
      items.push(["Reverse", "", () => songEdit(selClips().filter((k) => !k.pattern).map((k) => ["clip", k.id, { reverse: !k.reverse }]))]);
      items.push(["Select everything after", "", () => selectFrom(c)]);
      items.push(["Remove fades", "", () => songEdit(selClips().map((k) => ["clip", k.id, { fade_in: 0, fade_out: 0 }]))]);
      items.push({ gain: c });
    }
  } else {
    const pat = curPattern();
    if (pat) items.push([`Put "${pat.name}" here`, "dbl-click", () => insertPattern(p)]);
    items.push(["Paste here", "Ctrl+V", () => paste(Math.max(0, snapBeat(p.beat, snapFor(e))), p.row), !song.clipboard]);
    items.push(["Add a track", "", addTrack]);
    if (row) items.push([`Automate ${row.name || "this track"}…`, "", () => autoMenu(row.id, e.clientX, e.clientY)]);
    items.push(["Automate the master…", "", () => autoMenu("master", e.clientX, e.clientY)]);
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
const exp = { what: "mix", range: "song", norm: "" };
// the master's loudness, for the status bar (from the engine's state)
function lufsText(s) {
  const f = (x) => (x == null ? "–" : x.toFixed(1).replace("-", "−"));
  if (s.lufs_s == null && s.lufs_i == null) return "";
  const tp = s.true_peak;
  return `<span title="The master's loudness: short-term (3 s), integrated since you pressed play, true peak">` +
    `LUFS <b>${f(s.lufs_s)}</b> · I <b>${f(s.lufs_i)}</b> · TP <b${tp != null && tp > -1 ? ' class="hot"' : ""}>${f(tp)}</b></span>`;
}
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
  seg2("exWhat", "what"); seg2("exRange", "range"); seg2("exNorm", "norm");
  $("exNote").textContent = m.loop ? "" : "Turn on the loop to export just that part.";
  $("exProg").hidden = true; $("exShow").hidden = true; $("exGo").disabled = false;
  $("exportm").hidden = false;
}

function loudReport(l) {
  if (!l || l.integrated == null) return "";
  const f = (x) => (x == null ? "–" : x.toFixed(1).replace("-", "−"));
  const g = l.gain ? ` (turned ${l.gain > 0 ? "up" : "down"} ${Math.abs(l.gain).toFixed(1)} dB)` : "";
  return `The mix: ${f(l.integrated)} LUFS integrated, ${f(l.range)} LU range, true peak ${f(l.true_peak)} dBTP${g}.`;
}

async function runExport() {
  $("exGo").disabled = true;
  const res = await api.daw_export({ stems: exp.what === "stems", loop: exp.range === "loop", normalize: exp.norm === "" ? null : +exp.norm });
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
          + [loudReport(st.result.loudness), res.note, ...(st.result.warnings || [])].filter(Boolean).map((x) => " " + x).join("");
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

// ---------- mouse: ruler ----------
// what's under the mouse on the ruler: {lane: "marks" | "tempo" | "bars", beat, hit}
function rulerAt(ev) {
  const r = song.ruler.getBoundingClientRect(), x = ev.clientX - r.left, y = ev.clientY - r.top;
  const sx = $("lanes").scrollLeft, lane = y < RM ? "marks" : y < RT ? "tempo" : "bars";
  const hits = (song.rulerHits || []).filter((h) => x >= h.x0 && x <= h.x1 &&
    (lane === "marks" ? h.what !== "tag" : lane === "tempo" && h.what === "tag"));
  // a marker flag wins over the section band it's on
  const hit = hits.find((h) => h.what === "marker") || hits.find((h) => h.what === "tag") ||
    hits.find((h) => h.what === "section") || null;
  return { lane, beat: Math.max(0, (x + sx) / song.ppb), hit, x };
}
// the time signature list or the tempo list, with a change at beat b set (v) or taken out (null)
function setTag(kind, beat, v, from = beat) {
  const m = song.view.meta, field = kind === "tempo" ? "tempo" : "meters";
  if (beat <= 1e-9 && from <= 1e-9) {
    if (v != null) songEdit([["meta", "meta", kind === "tempo" ? { bpm: v } : { beats: v }]]);
    return;
  }
  const list = (m[field] || []).filter(([b]) => Math.abs(b - from) > 1e-6 && Math.abs(b - beat) > 1e-6);
  if (v != null && beat > 1e-9) list.push([beat, v]);
  list.sort((a, b) => a[0] - b[0]);
  songEdit([["meta", "meta", { [field]: list }]]);
}
function askTempo(beat, cur) {
  const s = prompt(`Tempo from bar ${fmtPos(beat).split(".")[0]} (BPM)`, String(+(+cur).toFixed(2)));
  if (s == null) return;
  const v = parseFloat(s.replace(",", "."));
  if (!(v >= 20 && v <= 400)) { toast("A tempo is between 20 and 400 BPM."); return; }
  setTag("tempo", beat, v);
}
function askMeter(beat, cur) {
  const s = prompt(`Beats per bar from bar ${fmtPos(beat).split(".")[0]} (e.g. 3 for 3/4, 7 for 7/4)`, String(cur));
  if (s == null) return;
  const v = parseInt(s.split("/")[0], 10);
  if (!(v >= 1 && v <= 32)) { toast("Between 1 and 32 beats per bar."); return; }
  setTag("meter", beat, v);
}
function addMarker(beat, section) {
  const name = prompt(section ? "Section name (e.g. Intro, Verse, Chorus)" : "Marker name", section ? "Verse" : "Marker");
  if (name == null) return;
  const id = newId("m");
  songEdit([["marker", id, { beat, name: name.trim() || (section ? "Section" : "Marker"), section: !!section }]]);
}
function selectSection(sc) {
  song.selSection = { id: sc.m.id };
  song.sel = new Set(song.view.clips.filter((c) => c.start >= sc.a - 1e-6 && c.start < sc.b - 1e-6).map((c) => c.id));
  songDraw(); drawRuler();
}
async function sectionOp(sc, op) {
  const n = Math.round((sc.b - sc.a) / bpbAt(sc.a) * 100) / 100;
  setSongView(await api.daw_time_op(op, sc.a, sc.b));
  song.selSection = null;
  toast(op === "delete" ? `"${sc.m.name}" is out (${n} bars); what came after moved up. Ctrl+Z brings it back.`
    : `"${sc.m.name}" is in twice now; what came after moved ${n} bars later.`);
}
function rulerContext(e) {
  e.preventDefault();
  if (!song.view) return;
  const p = rulerAt(e), at = Math.max(0, snapBeat(p.beat, snapFor(e))), bar = Math.max(0, snapBeat(p.beat, 0));
  const items = [], h = p.hit;
  if (h && (h.what === "marker" || h.what === "section")) {
    const m = h.m;
    items.push([`Rename "${m.name}"…`, "dbl-click", () => { const n = prompt("Name", m.name); if (n && n.trim()) songEdit([["marker", m.id, { name: n.trim() }]]); }]);
    items.push(["Color: next", "", () => songEdit([["marker", m.id, { color: MARK_COLORS[(MARK_COLORS.indexOf(markColor(m, markers().indexOf(m))) + 1) % MARK_COLORS.length] }]])]);
    if (h.what === "section") {
      const sc = h.sc;
      items.push(["Select the section", "click", () => selectSection(sc)]);
      items.push(["Loop the section", "", () => songEdit([["meta", "meta", { loop: true, loop_start: sc.a, loop_end: sc.b }]])]);
      items.push(["Duplicate the section", "", () => sectionOp(sc, "duplicate")]);
      items.push(["Delete the section (close the gap)", "", () => sectionOp(sc, "delete")]);
      items.push(["Make it a marker", "", () => songEdit([["marker", m.id, { section: false }]])]);
    } else items.push(["Make it a section", "", () => songEdit([["marker", m.id, { section: true }]])]);
    items.push([h.what === "section" ? "Remove the section marker" : "Remove the marker", "", () => songEdit([["marker", m.id, { del: true }]])]);
  } else if (h && h.what === "tag") {
    const t = h.t;
    if (t.kind === "tempo") items.push(["Change the tempo…", "dbl-click", () => askTempo(t.beat, t.v)]);
    else items.push(["Change the time signature…", "dbl-click", () => askMeter(t.beat, t.v)]);
    if (t.beat > 0) items.push([t.kind === "tempo" ? "Remove the tempo change" : "Remove the time signature change", "", () => setTag(t.kind, t.beat, null)]);
  } else {
    items.push(["Add a marker here", "", () => addMarker(at, false)]);
    items.push(["Start a section here", "", () => addMarker(bar, true)]);
    items.push([`Tempo change here (now ${+bpmAt(at).toFixed(2)})…`, "", () => askTempo(at, bpmAt(at))]);
    items.push([`Time signature from bar ${barOf(bar).bar + 1} (now ${bpbAt(bar)}/4)…`, "", () => askMeter(bar, bpbAt(bar))]);
    const sc = sections().find((x) => p.beat >= x.a && p.beat < x.b);
    if (sc) items.push([`Select "${sc.m.name}"`, "", () => selectSection(sc)]);
  }
  showCtx(items, e.clientX, e.clientY);
}
function rulerDbl(e) {
  if (!song.view) return;
  const p = rulerAt(e), h = p.hit;
  if (h && (h.what === "marker" || h.what === "section")) {
    const n = prompt("Name", h.m.name);
    if (n && n.trim()) songEdit([["marker", h.m.id, { name: n.trim() }]]);
  } else if (h && h.what === "tag") (h.t.kind === "tempo" ? askTempo : askMeter)(h.t.beat, h.t.v);
  else if (p.lane === "marks") addMarker(Math.max(0, snapBeat(p.beat, 0)), true);
  else if (p.lane === "tempo") askTempo(Math.max(0, snapBeat(p.beat, snapFor(e))), bpmAt(p.beat));
}
// drag a marker, a section's start, or a tempo/time signature change along the ruler
function rulerDragThing(e, p) {
  const h = p.hit, cv = song.ruler, x0 = e.clientX, isTag = h.what === "tag";
  if (isTag && h.t.beat <= 0) return false; // the start's tempo stays at the start
  let moved = false, beat = isTag ? h.t.beat : h.m.beat;
  const from = beat, meter = isTag && h.t.kind === "meter";
  if (isTag) song.tagDrag = { kind: h.t.kind, beat };
  cv.setPointerCapture(e.pointerId);
  const move = (ev) => {
    if (!moved && Math.abs(ev.clientX - x0) < 4) return;
    moved = true;
    const b = rulerAt(ev).beat;
    beat = Math.max(isTag ? 1e-3 : 0, snapBeat(b, meter || (!isTag && h.m.section) ? 0 : snapFor(ev)));
    if (isTag) {
      song.tagDrag.beat = beat;
      const m = song.view.meta, f = meter ? "meters" : "tempo";
      const prev = song.tagDrag.prev ?? from;
      m[f] = (m[f] || []).map(([b0, v]) => [Math.abs(b0 - prev) < 1e-6 ? beat : b0, v]);
      song.tagDrag.prev = beat;
    } else h.m.beat = beat;
    drawRuler(); songDraw();
  };
  const up = () => {
    cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up);
    song.tagDrag = null;
    if (!moved) {
      if (h.what === "section") selectSection(h.sc);
      return;
    }
    if (isTag) {
      const m = song.view.meta, f = meter ? "meters" : "tempo";
      m[f] = (m[f] || []).map(([b0, v]) => [Math.abs(b0 - beat) < 1e-6 ? from : b0, v]); // back; the edit sets it
      if (beat > 1e-3) setTag(h.t.kind, beat, h.t.v, from); else drawRuler();
    } else {
      h.m.beat = from;
      songEdit([["marker", h.m.id, { beat }]]);
    }
  };
  cv.addEventListener("pointermove", move); cv.addEventListener("pointerup", up);
  return true;
}
function rulerDown(e) {
  if (e.button !== 0 || !song.view) return;
  const rp = rulerAt(e);
  if (rp.hit && (rp.hit.what === "marker" || rp.hit.what === "tag" ||
      (rp.hit.what === "section" && Math.abs(rp.x - rp.hit.x0) <= 6)) && rulerDragThing(e, rp)) return;
  if (rp.hit && rp.hit.what === "section") {  // a click on a section's band selects it (and drags nothing)
    selectSection(rp.hit.sc); return;
  }
  if (song.selSection) { song.selSection = null; drawRuler(); }
  const cv = song.ruler, r = cv.getBoundingClientRect(), sx = $("lanes").scrollLeft;
  const at = (ev) => Math.max(0, snapBeat((ev.clientX - r.left + sx) / song.ppb, snapFor(ev)));
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
      <button id="mVersions"><span>Versions…</span></button>
      <button id="mSaveFile"><span>Save as a file…</span></button>
      <button id="mOpenFile"><span>Open a song file…</span></button>
      <button id="mExport"><span>Export as WAV…</span><kbd>Ctrl+E</kbd></button>
      <button id="mKeys"><span>Keyboard shortcuts…</span><kbd>Ctrl+/</kbd></button>
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
  $("mVersions").onclick = () => { menu.hidden = true; openVersions(); };
  $("mSaveFile").onclick = async () => {
    menu.hidden = true;
    const r = await api.daw_save_bundle();
    if (!r) return;
    if (r.error) toast(r.error, true);
    else toast(`Saved, with ${r.files} audio file${r.files === 1 ? "" : "s"}: ${r.path.split(/[\\/]/).pop()}`);
  };
  $("mOpenFile").onclick = async () => {
    if (v.shared && !confirm("Open a song file? Your partner switches to it too (this one stays saved).")) return;
    menu.hidden = true;
    const r = await api.daw_open_bundle();
    if (!r) return;
    if (r.error) toast(r.error, true);
    else setSongView(r.view);
  };
  $("mKeys").onclick = () => { menu.hidden = true; openShortcuts(); };
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

// ---------- versions: the song as it was, every few minutes while you work ----------
async function openVersions() {
  const list = await api.daw_versions();
  let m = $("versionsM");
  if (!m) {
    m = document.createElement("div");
    m.className = "modal";
    m.id = "versionsM";
    m.innerHTML = `<div class="mbox wide" role="dialog" aria-modal="true" aria-labelledby="verTitle">
      <div class="mhead"><b id="verTitle">Versions</b><span>Kept on this computer</span><button class="x" aria-label="Close">✕</button></div>
      <div class="mbody"><p class="fine">A version is kept every few minutes while the song changes (all of the last two hours, then one an hour,
      then one a day). Restoring one is an edit like any other: <b>Undo</b> takes it back, and your partner gets it too.</p>
      <div class="verlist"></div></div></div>`;
    document.body.append(m);
    m.querySelector(".x").onclick = () => (m.hidden = true);
    m.addEventListener("pointerdown", (e) => { if (e.target === m) m.hidden = true; });
  }
  const box = m.querySelector(".verlist");
  box.innerHTML = list.length ? "" : `<p class="fine">No versions yet: the first is kept a few minutes after you start changing the song.</p>`;
  let day = "";
  for (const ver of list) {
    const t = new Date(ver.time * 1000);
    const d = t.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" });
    if (d !== day) { day = d; box.insertAdjacentHTML("beforeend", `<div class="lbl-sm verday">${esc(d)}</div>`); }
    const row = document.createElement("div");
    row.className = "verrow";
    row.innerHTML = `<b>${t.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</b>
      <span>${esc(ver.name || "Untitled")} · ${ver.tracks} track${ver.tracks === 1 ? "" : "s"} · ${ver.clips} clip${ver.clips === 1 ? "" : "s"}${ver.notes ? ` · ${ver.notes} notes` : ""}</span>
      <button class="btn">Restore</button>`;
    row.querySelector("button").onclick = async () => {
      if (!confirm(`Put the song back as it was at ${t.toLocaleString()}? (Undo takes it back.)`)) return;
      const r = await api.daw_restore(ver.id);
      if (r.error) { toast(r.error, true); return; }
      setSongView(r.view);
      m.hidden = true;
      toast(`Restored the version from ${t.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}. Undo takes it back.`);
    };
    box.append(row);
  }
  m.hidden = false;
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

// the demo's sample library
function demoLibrary() {
  const pads = ["36 Kick", "37 Rim", "38 Snare", "39 Clap", "40 Snare 2", "41 Floor tom", "42 Closed hat", "43 Low tom", "44 Pedal hat", "45 Mid tom", "46 Open hat", "47 Tom", "48 High tom", "49 Crash", "50 Top tom", "51 Ride"];
  const f = (dir, name) => ({ name, path: `${dir}/${name}.wav`, size: 90000 });
  const dirs = {
    "/lib/Duet sounds": { folders: [{ name: "Drum kit", path: "/lib/Duet sounds/Drum kit" }], files: [] },
    "/lib/Duet sounds/Drum kit": { folders: [], files: pads.map((n) => f("/lib/Duet sounds/Drum kit", n)) },
    "/lib/My samples": { folders: [{ name: "Vocal chops", path: "/lib/My samples/Vocal chops" }], files: [f("/lib/My samples", "808 long"), f("/lib/My samples", "Vinyl crackle")], writable: true },
    "/lib/My samples/Vocal chops": { folders: [], files: ["Hey", "Oh yeah", "Chop 3"].map((n) => f("/lib/My samples/Vocal chops", n)), writable: true },
    "/packs/Lo-fi kit": { folders: [], files: ["Kick dusty", "Snare vinyl", "Hat soft", "Rim knock"].map((n) => f("/packs/Lo-fi kit", n)), writable: true },
  };
  const bins = { bfav: { name: "Favourites", parent: null, items: ["/lib/Duet sounds/Drum kit/36 Kick.wav", "/packs/Lo-fi kit/Snare vinyl.wav"] },
    bsub: { name: "Snares", parent: "bfav", items: ["/lib/Duet sounds/Drum kit/38 Snare.wav"] } };
  let prev = null, prevAt = 0;
  const binList = () => Object.entries(bins).map(([id, b], i) => ({ id, name: b.name, parent: b.parent, count: b.items.length, created: i }));
  const fileOf = (p) => ({ name: p.split("/").pop().replace(/\.wav$/, ""), path: p, size: 1 });
  return {
    lib_roots: async () => ({ roots: [{ path: "/lib/Duet sounds", name: "Duet sounds", kind: "sounds", writable: false },
      { path: "/lib/My samples", name: "My samples", kind: "mine", writable: true }, { path: "/packs/Lo-fi kit", name: "Lo-fi kit", kind: "folder", writable: true }],
      bins: binList(), recent: [fileOf("/lib/My samples/808 long.wav")] }),
    lib_list: async (p) => ({ path: p, writable: false, folders: [], files: [], ...(dirs[p] || { error: "That folder isn't there." }) }),
    lib_bin: async (id) => ({ bin: id, name: bins[id].name, bins: binList().filter((b) => b.parent === id), files: bins[id].items.map(fileOf) }),
    lib_search: async (q) => Object.values(dirs).flatMap((d) => d.files).filter((x) => x.name.toLowerCase().includes(q.toLowerCase())),
    lib_info: async (p) => {
      const n = 120, a = new Int8Array(2 * n);
      for (let i = 0; i < n; i++) { const v = Math.round(120 * Math.exp(-i / 18) * (0.6 + 0.4 * Math.sin(i * 1.7))); a[2 * i] = -v; a[2 * i + 1] = v; }
      let s = ""; for (const b of new Uint8Array(a.buffer)) s += String.fromCharCode(b);
      return { frames: 48000 * 0.6, sr: 48000, channels: p.includes("808") ? 2 : 1, peaks: btoa(s) };
    },
    lib_preview: async (p) => { prev = p; prevAt = performance.now(); return { ok: true }; },
    lib_preview_state: async () => { const t = (performance.now() - prevAt) / 600; return prev && t < 1 ? [t * 1000, 1000] : [0, 0]; },
    lib_new_bin: async (name, parent) => { const id = "b" + Math.random().toString(36).slice(2, 8); bins[id] = { name, parent: parent || null, items: [] }; return { ok: true, id }; },
    lib_bin_add: async (id, paths) => { for (const p of paths) if (!bins[id].items.includes(p)) bins[id].items.push(p); return { ok: true }; },
    lib_bin_remove: async (id, p) => { bins[id].items = bins[id].items.filter((x) => x !== p); },
    lib_rename_bin: async (id, n) => { bins[id].name = n; }, lib_delete_bin: async (id) => { delete bins[id]; }, lib_move_bin: async () => {},
    lib_import: async () => null, lib_add_folder: async () => null, lib_remove_folder: async () => null,
    lib_new_folder: async () => ({ error: "Not in the demo." }), lib_rename: async () => ({ error: "Not in the demo." }), lib_copy: async () => ({ ok: true }),
    lib_to_song: async () => ({ ok: true }), lib_to_channel: async () => ({ ok: true }),
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
      { id: "a1", name: "Guitar · Reverb · Mix", order: 1.5, owner: me, auto: { target: "t1", param: "fx:0:reverb:mix" },
        points: [[0, 0.1], [8, 0.1], [16, 0.7], [20, 0.7], [24, 0.25]], slot: null, mine: true, mute: false, solo: false },
    ],
    buses: [
      { id: "b1", name: "Reverb", order: 1001, owner: me, bus: true, vol: 0.8, pan: 0, rack: [{ kind: "reverb", size: 0.75, mix: 0.5, dry: 0 }], slot: 5, mine: true, mute: false, solo: false },
    ],
    clips: [
      { id: "c1", track: "t1", file: "take-g1.wav", start: 0, offset: 0, len: 15, gain: 1 },
      { id: "c2", track: "t2", file: "take-v1.wav", start: 8, offset: 0.5, len: 5, gain: 0.7, fade_in: 1.2, fade_out: 0.008, group: "gv", take: 2 },
      { id: "c2b", track: "t2", file: "take-v1.wav", start: 16, offset: 5.8, len: 6, gain: 0.7, fade_in: 0.008, fade_out: 2.5, group: "gv", take: 3 },
      { id: "c3", track: "t3", file: "take-k1.wav", start: 16, offset: 0, len: 8, gain: 1 },
      { id: "c4", track: "t4", pattern: "pa", start: 0, blen: 16, boff: 0 },
      { id: "c5", track: "t5", pattern: "pb", start: 8, blen: 16, boff: 0 },
    ],
    takes: [1, 2, 3].map((k) => ({ id: "tv" + k, track: "t2", file: "take-v1.wav", start: 8, offset: 0.2 + 0.3 * (k - 1), len: 11, gain: 0.7, group: "gv", take: k, tsrc: true })),
    patterns: [{ id: "pa", name: "Beat", len: 4 }, { id: "pb", name: "Bassline", len: 8 }],
    notes: [
      ...[0, 1.5, 2, 3.5].map((b, i) => ({ id: "k" + i, pattern: "pa", track: "t4", start: b, len: 0.25, key: 36, vel: 115 })),
      ...[1, 3].map((b, i) => ({ id: "s" + i, pattern: "pa", track: "t4", start: b, len: 0.25, key: 38, vel: 105 })),
      ...Array.from({ length: 8 }, (_, i) => ({ id: "h" + i, pattern: "pa", track: "t4", start: i / 2, len: 0.25, key: 42, vel: i % 2 ? 70 : 100 })),
      ...[[0, 45, 1.5], [1.5, 45, 0.5], [2, 48, 1], [3, 50, 1], [4, 41, 1.5], [5.5, 41, 0.5], [6, 43, 1], [7, 48, 1]].map(([b, k, l], i) => ({ id: "b" + i, pattern: "pb", track: "t5", start: b, len: l, key: k, vel: 100 })),
    ],
    markers: [{ id: "m1", beat: 0, name: "Intro", section: true }, { id: "m2", beat: 16, name: "Verse", section: true },
      { id: "m3", beat: 24, name: "drop the bass", section: false }],
    patternMode: null,
    files, me, partner, local: { click: true, countin: true, punch: false }, conflict: null, shared: true, undo: true, redo: false,
  };
  let v = 1, playing = false, recording = false, recFrom = null, pos = 0, t0 = 0, exportAt = 0, mockNorm = null;
  const copy = () => JSON.parse(JSON.stringify(view));
  const now = () => (playing ? s2b(b2s(pos) + (performance.now() - t0) / 1000) : pos);
  return {
    daw_project: async () => copy(),
    daw_edit: async (changes) => {
      for (const [kind, id, f] of changes) {
        if (kind === "meta") Object.assign(view.meta, f);
        const bus = kind === "track" && (f.bus || view.buses.some((b) => b.id === id));
        const list = { track: bus ? view.buses : view.tracks, clip: view.clips, pattern: view.patterns, note: view.notes, marker: view.markers }[kind];
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
    daw_comp: async (group, take, a, b) => {
      // (like daw.py's Comp, in beats; good enough for the demo)
      const cut = (c, x, y) => {
        const s0 = c.start, e0 = c.start + clipBeats(c), p = Math.max(x, s0), q = Math.min(y, e0);
        if (q - p < 1e-3) return null;
        return { ...c, start: p, offset: (c.offset || 0) + secsBetween(s0, p), len: secsBetween(p, q),
          fade_in: p > s0 ? 0.008 : c.fade_in || 0, fade_out: q < e0 ? 0.008 : c.fade_out || 0 };
      };
      for (const c of view.clips.filter((k) => k.group === group)) {
        const e0 = c.start + clipBeats(c);
        if (e0 <= a || c.start >= b) continue;
        const l = cut(c, -1e9, a), r = cut(c, b, 1e9);
        view.clips.splice(view.clips.indexOf(c), 1);
        if (l) view.clips.push(l);
        if (r) view.clips.push({ ...r, id: newId("c") });
      }
      for (const s of view.takes.filter((k) => k.group === group && k.take === take)) {
        const p = cut(s, a, b);
        if (p) { delete p.tsrc; view.clips.push({ ...p, id: newId("c") }); }
      }
      v++; return copy();
    },
    daw_take_op: async (group, op, take) => {
      if (op === "flatten") {
        view.takes = view.takes.filter((k) => k.group !== group);
        for (const c of view.clips) if (c.group === group) { delete c.group; delete c.take; }
      } else if (op === "delete") {
        view.takes = view.takes.filter((k) => !(k.group === group && k.take === take));
        view.clips = view.clips.filter((k) => !(k.group === group && k.take === take));
      }
      v++; return copy();
    },
    daw_time_op: async (op, a, b) => {
      // (the real one also moves automation, tempo and time signatures)
      const L = b - a, dup = op === "duplicate";
      for (const [list, f] of [[view.clips, "start"], [view.markers, "beat"]]) {
        for (const o of [...list]) {
          const x = o[f];
          if (dup && x >= a && x < b) list.push({ ...o, id: newId("d"), [f]: x + L });
          if (x >= b) o[f] = dup ? x + L : x - L;
          else if (x >= a && !dup) list.splice(list.indexOf(o), 1);
        }
      }
      v++; return copy();
    },
    daw_local: async (k, val, tid) => {
      if (k === "livesends") view.local.livesends = { ...(view.local.livesends || {}), [tid]: val };
      else if (tid) view.tracks.find((t) => t.id === tid)[k] = val; else view.local[k] = val;
      v++; return copy();
    },
    daw_play: async () => { pos = now(); playing = true; t0 = performance.now(); view.patternMode = null; },
    daw_play_pattern: async (pid) => { pos = 0; playing = true; t0 = performance.now(); view.patternMode = pid; v++; },
    daw_stop: async () => { playing = false; recording = false; pos = 0; view.patternMode = null; v++; },
    daw_record: async () => { playing = true; recording = true; recFrom = pos; t0 = performance.now(); return { ok: true }; },
    daw_locate: async (p) => { pos = p; t0 = performance.now(); },
    daw_peaks: async (n) => pk[n] || null,
    stems_state: async () => ({ model: false, size: 174264716, download: null, job: null }),
    stems_get_model: async () => ({ ok: true }),
    stems_split: async () => ({ error: "The demo has no stem model." }),
    stems_cancel: async () => {},
    // (the demo makes up its answers: a steady beat at 96, hits every half second)
    daw_analyze: async (id, what) => {
      const c = view.clips.find((k) => k.id === id);
      if (!c) return { error: "That clip is gone." };
      if (what === "peak") return { peak: 0.5 };
      if (what === "tempo") return { bpm: 96 * clipRatio(c) };
      if (what === "key") return { key: "A minor", confidence: 0.7, others: ["C major", "E minor"] };
      if (what === "pitch") {
        // a made-up voice: a phrase in A minor, some notes a little off, with vibrato
        const hop = 0.0058, o = c.offset || 0, L = c.len * clipRatio(c), midi = [], rms = [], notes = [];
        const plan = [[69.2, 0.5], [71.7, 0.4], [72.1, 0.8], [74.3, 0.5], [76, 1.0], [74.6, 0.4], [72, 0.6], [71.4, 0.5], [69.1, 1.2], [68.5, 0.5], [69, 0.9]];
        let t = o + 0.2;
        const at = [];
        for (const [m, d] of plan) { if (t + d > o + L) break; at.push([t, t + d, m]); t += d + 0.12; }
        for (let k = 0; o + k * hop < o + L; k++) {
          const tt = o + k * hop, n = at.find((x) => tt >= x[0] && tt < x[1]);
          if (!n) { midi.push(null); rms.push(0.02); continue; }
          const u = (tt - n[0]) / (n[1] - n[0]);
          midi.push(+(n[2] + 0.18 * Math.sin(2 * Math.PI * 5.5 * (tt - n[0])) * Math.min(1, u * 3) - 0.3 * Math.max(0, 0.15 - u) / 0.15).toFixed(3));
          rms.push(+(0.4 + 0.6 * Math.sin(Math.PI * u)).toFixed(3));
        }
        for (const [a, b, m] of at) notes.push([a, b, m]);
        return { pitch: { hop, start: o, midi, rms, notes } };
      }
      const o = c.offset || 0, L = c.len * clipRatio(c), hits = [];
      for (let t = Math.ceil(o / 0.5) * 0.5 + 0.02; t < o + L; t += 0.5) hits.push(t);
      return { hits };
    },
    daw_versions: async () => [0, 7, 19, 64, 1500].map((min, i) => ({ id: "v" + i, time: Date.now() / 1000 - min * 60 - 30, clips: view.clips.length - i, notes: 12, tracks: view.tracks.length, name: view.meta.name })),
    daw_restore: async () => ({ ok: true, changed: 3, view: copy() }),
    daw_save_bundle: async () => ({ ok: true, files: 4, path: "/Users/you/Music/Summer jam.duet" }),
    daw_open_bundle: async () => null,
    daw_projects: async () => [{ pid: "pdemo", name: view.meta.name, modified: Date.now() / 1000, clips: view.clips.length, current: true },
      { pid: "p2", name: "Blues in A", modified: Date.now() / 1000 - 86400, clips: 5, current: false }],
    daw_new: async () => copy(), daw_open: async () => copy(), daw_resolve: async () => copy(),
    daw_undo: async () => copy(), daw_redo: async () => copy(),
    daw_export: async (o) => { mockNorm = o.normalize; exportAt = performance.now(); return { ok: true, note: "", files: o.stems ? ["/Users/you/Music/Summer jam (stems)/Summer jam.wav", "a", "b", "c"] : ["/Users/you/Music/Summer jam.wav"] }; },
    daw_export_status: async () => {
      const p = Math.min(1, (performance.now() - exportAt) / 1500);
      return p < 1 ? { running: true, progress: p, result: null } : { running: false, progress: 1, result: { ok: true, warnings: [],
        loudness: mockNorm == null ? { integrated: -11.3, range: 6.4, true_peak: 0.4, gain: null } : { integrated: mockNorm, range: 6.4, true_peak: mockNorm + 11.7, gain: mockNorm + 11.3 } } };
    },
    show_file: async () => {},
    daw_presence: async () => {},
    ...demoLibrary(),
    status: () => ({ v, pos: now(), playing, recording, recFrom,
      levels: playing ? { t1: 0.4, t2: 0.25 } : {}, xfer: { "take-k1.wav": [3_100_000, 4_600_000, "in"] }, notices: [],
      ...demoPartner() }),
  };
}

// the bar over the playlist while it's locked (a split into stems)
function lockBanner() {
  const j = song.st && song.st.stems && song.st.stems.job;
  let el = $("plLock");
  if (!j) { if (el) el.hidden = true; return; }
  if (!el) {
    el = document.createElement("div");
    el.id = "plLock";
    el.className = "pllock";
    el.innerHTML = '<span></span><button class="btn sq">Stop</button>';
    el.querySelector("button").onclick = () => api.stems_cancel();
    $("lanewrap").append(el);
  }
  el.hidden = false;
  el.querySelector("span").textContent = "🔒 Making stems" + (j.stage === "splitting" ? ` · ${Math.round((j.progress || 0) * 100)}%` : j.stage === "writing" ? " · writing" : "…")
    + " — the playlist is locked until it's done; the rest of Duet works as usual.";
}

// one round of a pattern when its block loops (as daw.py's _round): its base length (or its length),
// longer if its notes go on past that (to the bar)
function patRound(p) {
  if (!p) return 4 * bpb();
  let r = p.base || patLen(p), end = 0;
  for (const n of song.view.notes || []) if (n.pattern === p.id) end = Math.max(end, n.start + n.len);
  if (end > r + 1e-6) r = Math.ceil(end / bpb() - 1e-9) * bpb();
  return Math.max(0.25, r);
}
