// Automation lanes in the playlist (FL's automation clips, as a row each).
//
// A lane is a song track with `auto: {target, param}` and `points: [[beat,
// value 0..1], ...]`, joined by straight lines. target is a track id or
// "master"; param is "vol", "pan", "fx:<pos>:<kind>:<knob>" or "inst:<knob>".
// Both of you hear it (it's part of the song), and it goes into the export.
//
// In the playlist: click to add a point, drag one to move it, right-click to
// take it away (or Alt-click). Make one from any knob's right-click menu, a
// track's header (right-click), or the empty space of the playlist.

const AUTO_COLOR = "#ffc857";
const AUTO_PAD = 7; // px above and below the line inside its row
const AUTO_HIT = 7;

const isAuto = (t) => !!(t && t.auto);

// what a lane can move on a target: [[param, label, value now (0..1)], ...]
function autoParams(target) {
  const v = song.view, out = [];
  if (target === "master") {
    const rack = v.meta.master_rack || [];
    rack.forEach((slot, i) => fxParams(slot, i, out));
    return out;
  }
  const t = [...v.tracks, ...(v.buses || [])].find((x) => x.id === target);
  if (!t || t.auto) return out;
  out.push(["vol", "Volume", faderPos(t.vol ?? 0.8)], ["pan", "Pan", ((t.pan ?? 0) + 1) / 2]);
  (t.rack || []).forEach((slot, i) => fxParams(slot, i, out));
  const meta = t.inst && INST[t.inst.kind];
  if (meta) for (const [key, label] of meta.knobs) out.push([`inst:${key}`, `${meta.name} · ${label}`, t.inst[key] ?? INST_DEFAULTS[t.inst.kind]?.[key] ?? 0.5]);
  return out;
}

function fxParams(slot, i, out) {
  const meta = FX.find((f) => f.id === slot.kind);
  if (!meta) return;
  for (const [key, label] of meta.knobs) out.push([`fx:${i}:${slot.kind}:${key}`, `${meta.name} · ${label}`, slot[key] ?? FX_DEFAULTS[slot.kind]?.[key] ?? 0.5]);
}

function autoTargetName(target) {
  if (target === "master") return "Master";
  const t = [...song.view.tracks, ...(song.view.buses || [])].find((x) => x.id === target);
  return t ? t.name || "Track" : "(deleted track)";
}

function autoLabel(a) {
  const p = autoParams(a.target).find((x) => x[0] === a.param);
  return p ? p[1] : a.param.startsWith("fx:") ? `${a.param.split(":")[2]} · ${a.param.split(":")[3]}` : a.param;
}

// a new lane right under what it moves, starting flat at the value it has now
function addAutomation(target, param) {
  const v = song.view;
  const p = autoParams(target).find((x) => x[0] === param);
  if (!p) { toast("There's nothing to automate there."); return; }
  const dup = v.tracks.find((t) => t.auto && t.auto.target === target && t.auto.param === param);
  if (dup) { setMainView("song"); song.selTrack = dup.id; renderHeads(); scrollToRow(trackRow(dup.id)); return; }
  // right under the track it moves (after its other lanes), the master's at the end
  const orders = v.tracks.map((t) => +t.order || 0), from = v.tracks.find((t) => t.id === target);
  const lanes = v.tracks.filter((t) => t.auto && t.auto.target === target).map((t) => +t.order || 0);
  const here = from ? Math.max(+from.order || 0, ...lanes) : Math.max(0, ...orders);
  const next = Math.min(...orders.filter((o) => o > here));
  const order = from && Number.isFinite(next) ? (here + next) / 2 : here + 1;
  const id = newId("a");
  songEdit([["track", id, { name: `${autoTargetName(target)} · ${p[1]}`, order, owner: v.me, auto: { target, param },
    points: [[0, +p[2].toFixed(4)]] }]]);
  song.selTrack = id;
  setMainView("song");
}

function scrollToRow(row) {
  const lanes = $("lanes");
  if (row >= 0 && lanes) lanes.scrollTop = Math.max(0, row * ROW - lanes.clientHeight / 2);
}

// a menu of everything on a target that can have a lane
function autoMenu(target, x, y) {
  const items = autoParams(target).map(([param, label]) => [`Automate ${label}`, "", () => addAutomation(target, param)]);
  if (!items.length) items.push(["Nothing to automate here", "", () => {}, true]);
  showCtx(items, x, y);
}

// right-click on a knob or fader of a song strip ("T<slot>" or "M"): Automate
function autoOnControl(el, strip, param) {
  if (!el || !(strip === "M" || /^T\d+$/.test(strip))) return el;
  el.addEventListener("contextmenu", (e) => {
    if (!song.view) return;
    const target = strip === "M" ? "master" : trackBySlot(+strip.slice(1))?.id;
    if (!target) return;
    e.preventDefault();
    e.stopPropagation();
    const p = autoParams(target).find((x) => x[0] === param || (param.startsWith("fx:") && x[0].startsWith(param + ":")));
    if (!p) return;
    const lane = song.view.tracks.find((t) => t.auto && t.auto.target === target && t.auto.param === p[0]);
    showCtx([[lane ? `Show the automation of ${p[1]}` : `Automate ${p[1]}`, "", () => addAutomation(target, p[0])]], e.clientX, e.clientY);
  });
  return el;
}

// --- the lane in the playlist -------------------------------------------------------

const autoY = (v, y) => y + AUTO_PAD + (1 - v) * (ROW - 2 * AUTO_PAD);
const autoV = (py, y) => Math.max(0, Math.min(1, 1 - (py - y - AUTO_PAD) / (ROW - 2 * AUTO_PAD)));

function autoPoints(t) {
  const d = song.autoDrag;
  return d && d.id === t.id ? d.points : (t.points || []).slice().sort((a, b) => a[0] - b[0]);
}

function drawAutoLane(ctx, t, y, sx, w) {
  const ppb = song.ppb, pts = autoPoints(t), bad = t.autoBad;
  const col = bad ? "#8a8494" : AUTO_COLOR;
  ctx.save();
  ctx.fillStyle = "rgba(255,200,87,0.035)";
  ctx.fillRect(0, y + 1, w, ROW - 2);
  if (!pts.length) { ctx.restore(); return; }
  const xs = pts.map((p) => p[0] * ppb - sx), ys = pts.map((p) => autoY(p[1], y));
  // filled area under the line
  ctx.beginPath();
  ctx.moveTo(0, y + ROW - 1);
  ctx.lineTo(0, ys[0]);
  for (let i = 0; i < pts.length; i++) ctx.lineTo(xs[i], ys[i]);
  ctx.lineTo(w, ys[ys.length - 1]);
  ctx.lineTo(w, y + ROW - 1);
  ctx.closePath();
  ctx.fillStyle = bad ? "rgba(138,132,148,0.10)" : "rgba(255,200,87,0.12)";
  ctx.fill();
  ctx.beginPath();
  ctx.moveTo(Math.min(0, xs[0]), ys[0]);
  for (let i = 0; i < pts.length; i++) ctx.lineTo(xs[i], ys[i]);
  ctx.lineTo(Math.max(w, xs[xs.length - 1]), ys[ys.length - 1]);
  ctx.strokeStyle = col;
  ctx.lineWidth = 2;
  ctx.stroke();
  const hov = song.autoHover;
  for (let i = 0; i < pts.length; i++) {
    if (xs[i] < -8 || xs[i] > w + 8) continue;
    const on = hov && hov.id === t.id && hov.i === i;
    ctx.beginPath();
    ctx.arc(xs[i], ys[i], on ? 5.5 : 4, 0, Math.PI * 2);
    ctx.fillStyle = on ? "#fff" : "#1a1720";
    ctx.fill();
    ctx.strokeStyle = col;
    ctx.lineWidth = 2;
    ctx.stroke();
  }
  if (bad) {
    ctx.fillStyle = "#b9b2c4";
    ctx.font = "11px system-ui, sans-serif";
    ctx.fillText("What this lane moves isn't there any more", 10, y + 16);
  }
  ctx.restore();
}

function autoHit(t, p) {
  const pts = autoPoints(t), y = p.row * ROW;
  for (let i = pts.length - 1; i >= 0; i--) {
    if (Math.abs(pts[i][0] * song.ppb - p.px) <= AUTO_HIT && Math.abs(autoY(pts[i][1], y) - p.py) <= AUTO_HIT) return i;
  }
  return -1;
}

function autoHoverAt(t, p) {
  const i = t ? autoHit(t, p) : -1;
  const h = i >= 0 ? { id: t.id, i } : null;
  $("lanes").style.cursor = h ? "move" : t ? "crosshair" : "";
  if ((h && h.id + h.i) !== (song.autoHover && song.autoHover.id + song.autoHover.i)) { song.autoHover = h; songDraw(); }
}

function autoSave(t, points) {
  t.points = points;
  throttledEdit("track", t.id, { points });
}

function autoRemovePoint(t, i) {
  const pts = autoPoints(t).filter((_, k) => k !== i);
  autoSave(t, pts);
  song.autoHover = null;
  songDraw();
}

function autoDown(e, p, t) {
  const lanes = $("lanes"), y = p.row * ROW;
  let pts = autoPoints(t).map((q) => q.slice());
  let i = autoHit(t, p);
  if (i >= 0 && e.altKey) { autoRemovePoint(t, i); return; }
  const at = (q) => Math.max(0, e.altKey || q.alt ? q.beat : snapBeat(q.beat, song.snap === 0 ? 1 : song.snap));
  if (i < 0) {
    pts.push([at(p), +autoV(p.py, y).toFixed(4)]);
    pts.sort((a, b) => a[0] - b[0]);
    i = pts.findIndex((q) => q[0] === at(p) && Math.abs(q[1] - autoV(p.py, y)) < 1e-3);
  }
  const d = { id: t.id, points: pts, i, x0: e.clientX, y0: e.clientY };
  song.autoDrag = d;
  song.autoHover = { id: t.id, i };
  songDraw();
  const move = (ev) => {
    const q = lanePoint(ev);
    q.alt = ev.altKey;
    // a point stays between its neighbours (two can share a beat: a jump)
    const lo = d.i > 0 ? d.points[d.i - 1][0] : 0, hi = d.i < d.points.length - 1 ? d.points[d.i + 1][0] : Infinity;
    d.points[d.i] = [Math.min(hi, Math.max(lo, at(q))), +autoV(q.py, y).toFixed(4)];
    showAutoValue(t, d.points[d.i][1], ev);
    songDraw();
  };
  const up = () => {
    lanes.removeEventListener("pointermove", move);
    lanes.removeEventListener("pointerup", up);
    song.autoDrag = null;
    hideAutoValue();
    autoSave(t, d.points);
    songDraw();
  };
  lanes.addEventListener("pointermove", move);
  lanes.addEventListener("pointerup", up);
}

// what the point is at, as the knob would say it
function autoFmt(t, v) {
  const a = t.auto, p = a.param;
  if (p === "vol") return fmtDb(faderGain(v));
  if (p === "pan") return fmtPan(2 * v - 1);
  const knobs = p.startsWith("fx:") ? FX.find((f) => f.id === p.split(":")[2])?.knobs
    : (() => { const tr = song.view.tracks.find((x) => x.id === a.target); return tr?.inst && INST[tr.inst.kind]?.knobs; })();
  const key = p.split(":").pop(), k = (knobs || []).find((x) => x[0] === key);
  return k && k[2] ? k[2](v) : Math.round(v * 100) + "%";
}

function showAutoValue(t, v, ev) {
  let tip = $("autoTip");
  if (!tip) { tip = document.createElement("div"); tip.id = "autoTip"; tip.className = "autotip"; document.body.append(tip); }
  tip.textContent = autoFmt(t, v);
  tip.hidden = false;
  tip.style.left = ev.clientX + 14 + "px";
  tip.style.top = ev.clientY - 10 + "px";
}
function hideAutoValue() { const tip = $("autoTip"); if (tip) tip.hidden = true; }

function autoContext(p, t, x, y) {
  const i = autoHit(t, p), items = [];
  if (i >= 0) items.push(["Delete this point", "Alt+click", () => autoRemovePoint(t, i)]);
  const v = autoParams(t.auto.target).find((q) => q[0] === t.auto.param);
  items.push(["Make it flat (one point)", "", () => { const pts = autoPoints(t); autoSave(t, [[0, pts.length ? pts[0][1] : v ? +v[2].toFixed(4) : 0.5]]); songDraw(); }]);
  items.push(["Delete this lane", "", () => songEdit([["track", t.id, { del: true }]])]);
  showCtx(items, x, y);
}

// the lane's header: its name, what it moves (a menu to point it elsewhere on
// the same track), delete
function autoHead(t, el) {
  el.classList.add("auto");
  const name = el.querySelector(".tn");
  const btns = document.createElement("div");
  btns.className = "tbtns";
  const del = document.createElement("button");
  del.className = "del"; del.textContent = "✕"; del.title = "Delete this automation lane";
  del.onclick = () => songEdit([["track", t.id, { del: true }]]);
  btns.append(del);
  const row2 = document.createElement("div");
  row2.className = "row2";
  const sel = document.createElement("select");
  sel.className = "tsrc autosel";
  sel.title = "What this lane moves";
  const params = autoParams(t.auto.target);
  if (!params.some((q) => q[0] === t.auto.param)) sel.append(new Option(`${autoTargetName(t.auto.target)} · ${autoLabel(t.auto)} (gone)`, t.auto.param, false, true));
  for (const [param, label] of params) sel.append(new Option(`${autoTargetName(t.auto.target)} · ${label}`, param, false, param === t.auto.param));
  sel.onfocus = () => (song.headBusy = true);
  sel.onblur = () => { song.headBusy = false; if (song.pendingHeads) renderHeads(); };
  sel.onchange = () => songEdit([["track", t.id, { auto: { target: t.auto.target, param: sel.value } }]]);
  row2.append(sel);
  el.append(name, btns, row2);
}
