// Duet — takes and comping. A take folder is the clips of one group: each take's whole recording
// (song.view.takes, silent) and the comp (ordinary clips, each from one take). In the clip editor's
// Takes tab: drag along a take to have that take play there; click a take to swap it in for the part
// of the comp under the mouse.

const TAKE_COLORS = ["#ff9548", "#4fd6c6", "#b48cff", "#ffd166", "#ff6b8b", "#6bb6ff", "#8fdc6a", "#f0a3ff"];
const takeColor = (k) => TAKE_COLORS[(k - 1) % TAKE_COLORS.length];
const TK_ROW = 46, TK_COMP = 30, TK_LEFT = 74;
const tk = { drag: null, hover: null };

function takeGroup(group) {
  const v = song.view;
  const srcs = (v.takes || []).filter((c) => c.group === group).sort((a, b) => a.take - b.take || a.start - b.start);
  const comp = v.clips.filter((c) => c.group === group).sort((a, b) => a.start - b.start);
  const nums = [...new Set(srcs.map((c) => c.take))].sort((a, b) => a - b);
  const all = [...srcs, ...comp];
  const a = Math.min(...all.map((c) => c.start)), b = Math.max(...all.map((c) => c.start + clipBeats(c)));
  return { srcs, comp, nums, a, b };
}

function takesTab(c) {
  const box = document.createElement("div");
  if (!c.group) {
    const p = document.createElement("p");
    p.className = "fine";
    p.textContent = "This clip has no other takes. Record over it (Punch, or with the loop on) and the takes land here to choose from.";
    box.append(p);
    return box;
  }
  const g = takeGroup(c.group);
  const hint = document.createElement("p");
  hint.className = "fine";
  hint.textContent = "Drag along a take: that take plays there. Click a take: it takes over the part of the comp under the mouse. Right-click a take to delete it.";
  const cv = document.createElement("canvas");
  cv.id = "takesCv";
  cv.className = "takescv";
  cv.style.height = TK_COMP + g.nums.length * TK_ROW + 8 + "px";
  cv.addEventListener("pointerdown", takesDown);
  cv.addEventListener("pointermove", (e) => { tk.hover = takesAt(e); takesDraw(); });
  cv.addEventListener("pointerleave", () => { tk.hover = null; takesDraw(); });
  cv.addEventListener("contextmenu", takesContext);
  const btns = document.createElement("div");
  btns.className = "cebtns";
  btns.append(ceBtn("Flatten", "Keep the comp as it is, as plain clips; the takes go", async () => {
    setSongView(await api.daw_take_op(c.group, "flatten"));
    toast("Flattened: the comp stays, the other takes are gone (Ctrl+Z brings them back).");
  }));
  for (const k of g.nums) btns.append(ceBtn(`Take ${k}`, `Take ${k} all the way`, () => compSet(c.group, k, g.a, g.b), "btn sq"));
  box.append(hint, cv, ceRow("Use", btns));
  requestAnimationFrame(takesDraw);
  return box;
}

async function compSet(group, take, a, b) {
  if (!(b > a)) return;
  setSongView(await api.daw_comp(group, take, a, b));
  // the clip the editor shows may have been cut away: follow the comp
  if (ce.id && !song.view.clips.some((k) => k.id === ce.id)) {
    const c = song.view.clips.find((k) => k.group === group);
    if (c) ce.id = c.id;
  }
  ceRender();
}

// the canvas: x in beats, rows: the comp, then a row a take
function takesGeom() {
  const cv = $("takesCv"), c = ceClip();
  if (!cv || !c || !c.group) return null;
  const g = takeGroup(c.group), w = cv.clientWidth, pad = (g.b - g.a) * 0.03 + 0.01;
  const a = g.a - pad, b = g.b + pad, x = (beat) => TK_LEFT + (beat - a) / (b - a) * (w - TK_LEFT - 6);
  return { cv, c, g, w, a, b, x, beat: (px) => a + (px - TK_LEFT) / (w - TK_LEFT - 6) * (b - a) };
}
function takesAt(e) {
  const G = takesGeom();
  if (!G) return null;
  const r = G.cv.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
  const row = py < TK_COMP ? -1 : Math.floor((py - TK_COMP) / TK_ROW);
  return { take: row >= 0 && row < G.g.nums.length ? G.g.nums[row] : null, beat: G.beat(px), px };
}

function takesDraw() {
  const G = takesGeom();
  if (!G) return;
  const { cv, g, w, x } = G, h = TK_COMP + g.nums.length * TK_ROW + 8;
  const ctx = sizeCanvas(cv, w, h, false);
  ctx.fillStyle = "#0e0d12"; ctx.fillRect(0, 0, w, h);
  ctx.font = "600 11px system-ui, sans-serif";
  // the comp: which take plays where
  ctx.fillStyle = "#cfc8da"; ctx.fillText("Comp", 10, TK_COMP / 2 + 4);
  for (const c of g.comp) {
    const x0 = x(c.start), x1 = x(c.start + clipBeats(c));
    ctx.fillStyle = takeColor(c.take) + "c0";
    ctx.fillRect(x0, 6, Math.max(1, x1 - x0 - 1), TK_COMP - 12);
    ctx.fillStyle = "#16141c";
    if (x1 - x0 > 26) ctx.fillText(String(c.take), x0 + 5, TK_COMP / 2 + 4);
  }
  g.nums.forEach((k, i) => {
    const y = TK_COMP + i * TK_ROW, col = takeColor(k);
    ctx.fillStyle = i % 2 ? "#131118" : "#16141c"; ctx.fillRect(0, y, w, TK_ROW);
    ctx.fillStyle = col; ctx.fillText(`Take ${k}`, 10, y + TK_ROW / 2 + 4);
    for (const s of g.srcs.filter((s) => s.take === k)) {
      const x0 = x(s.start), x1 = x(s.start + clipBeats(s));
      ctx.fillStyle = col + "1c"; ctx.fillRect(x0, y + 3, x1 - x0, TK_ROW - 6);
      takeWave(ctx, s, x0, x1, y + 3, TK_ROW - 6, x, (beat) => {
        // bright where this take is in the comp
        return g.comp.some((c) => c.take === k && beat >= c.start - 1e-6 && beat < c.start + clipBeats(c) + 1e-6);
      }, col);
    }
  });
  // what a drag would pick
  const d = tk.drag, hv = tk.hover;
  if (d && d.moved) {
    const i = g.nums.indexOf(d.take), y = TK_COMP + i * TK_ROW;
    ctx.strokeStyle = "#fff"; ctx.lineWidth = 1.5;
    ctx.strokeRect(x(Math.min(d.a, d.b)), y + 2, x(Math.max(d.a, d.b)) - x(Math.min(d.a, d.b)), TK_ROW - 4);
  } else if (hv && hv.take != null) {
    const seg = g.comp.find((c) => hv.beat >= c.start && hv.beat < c.start + clipBeats(c));
    if (seg) {
      const i = g.nums.indexOf(hv.take), y = TK_COMP + i * TK_ROW;
      ctx.strokeStyle = "#ffffff80"; ctx.lineWidth = 1;
      ctx.strokeRect(x(seg.start) + 0.5, y + 2.5, x(seg.start + clipBeats(seg)) - x(seg.start) - 1, TK_ROW - 5);
    }
  }
  // the playhead
  const pos = curPos();
  if (pos >= G.a && pos <= G.b) { ctx.fillStyle = "#fff"; ctx.fillRect(x(pos), 0, 1.5, h); }
}

// a take's waveform from its file's peaks
function takeWave(ctx, s, x0, x1, y, h, x, lit, col, left = TK_LEFT) {
  const levels = song.peaks[s.file], f = song.view.files[s.file] || {};
  if (!levels) {
    if (f.here && !song.peakReq[s.file]) {
      song.peakReq[s.file] = true;
      api.daw_peaks(s.file).then((bb) => { if (bb) { song.peaks[s.file] = peakLevels(decodePeaks(bb)); takesDraw(); songDraw(); } else song.peakReq[s.file] = false; });
    }
    return;
  }
  const sr = f.sr || 48000, s0 = b2s(s.start), beatOf = (px) => s.start + (px - x0) / (x1 - x0) * clipBeats(s);
  const secPx = (b2s(s.start + clipBeats(s)) - s0) / Math.max(1, x1 - x0);
  let k = 0;
  while (k + 1 < levels.length && (512 << (k + 1)) <= secPx * sr) k++;
  const pk = levels[k], per = 512 << k, mid = y + h / 2, gn = (h / 2 - 2) / 127;
  for (let px = Math.max(left, Math.floor(x0)); px < Math.min(x1, left + 1e5); px++) {
    const beat = beatOf(px), t = clipSrcAt(s, b2s(beat) - s0);
    const i0 = Math.max(0, Math.floor(t * sr / per)), i1 = Math.max(i0 + 1, Math.floor((t + secPx) * sr / per));
    let lo = 0, hi = 0;
    for (let i = i0; i < i1 && 2 * i + 1 < pk.length; i++) { lo = Math.min(lo, pk[2 * i]); hi = Math.max(hi, pk[2 * i + 1]); }
    ctx.fillStyle = lit(beat) ? col + "f0" : "#5a5466";
    ctx.fillRect(px, mid - hi * gn, 1, Math.max(1, (hi - lo) * gn));
  }
}

function takesDown(e) {
  if (e.button !== 0) return;
  const p = takesAt(e), G = takesGeom();
  if (!p || p.take == null || !G) return;
  const cv = G.cv, at = (ev) => Math.max(G.g.a, Math.min(G.g.b, snapBeat(takesAt(ev).beat, snapFor(ev))));
  tk.drag = { take: p.take, a: at(e), b: at(e), x0: e.clientX, moved: false };
  cv.setPointerCapture(e.pointerId);
  const move = (ev) => {
    if (Math.abs(ev.clientX - tk.drag.x0) > 3) tk.drag.moved = true;
    tk.drag.b = at(ev);
    takesDraw();
  };
  const up = (ev) => {
    cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up);
    const d = tk.drag;
    tk.drag = null;
    if (d.moved) { compSet(G.c.group, d.take, Math.min(d.a, d.b), Math.max(d.a, d.b)); return; }
    // a click: this take for the part of the comp under the mouse (or, in a gap, where the take is)
    const beat = takesAt(ev).beat, g = takeGroup(G.c.group);
    const seg = g.comp.find((c) => beat >= c.start && beat < c.start + clipBeats(c));
    const src = g.srcs.find((s) => s.take === d.take && beat >= s.start && beat < s.start + clipBeats(s));
    if (seg) compSet(G.c.group, d.take, seg.start, seg.start + clipBeats(seg));
    else if (src) compSet(G.c.group, d.take, src.start, src.start + clipBeats(src));
  };
  cv.addEventListener("pointermove", move); cv.addEventListener("pointerup", up);
}

function takesContext(e) {
  e.preventDefault();
  const p = takesAt(e), G = takesGeom();
  if (!p || p.take == null || !G) return;
  showCtx([
    [`Take ${p.take} all the way`, "", () => compSet(G.c.group, p.take, G.g.a, G.g.b)],
    [`Delete take ${p.take}`, "", async () => { setSongView(await api.daw_take_op(G.c.group, "delete", p.take)); ceRender(); }],
  ], e.clientX, e.clientY);
}

// the playlist's right-click menu on a clip in a take folder
function takeMenuItems(c) {
  if (!c.group) return [];
  const g = takeGroup(c.group), end = c.start + clipBeats(c), items = [];
  items.push(["Takes…", "", () => { openClipEditor(c.id); ce.tab = "takes"; ceRender(); }]);
  for (const k of g.nums) if (k !== c.take) items.push([`Swap in take ${k} here`, "", async () => setSongView(await api.daw_comp(c.group, k, c.start, end))]);
  items.push(["Flatten takes", "", async () => setSongView(await api.daw_take_op(c.group, "flatten"))]);
  return items;
}

// ---------- take lanes in the playlist: under a track whose takes are open (▸ in its header) ----------
function toggleTakes(tid) {
  song.takesOpen ||= new Set();
  song.takesOpen.has(tid) ? song.takesOpen.delete(tid) : song.takesOpen.add(tid);
  try { localStorage.setItem("duet.takesOpen", JSON.stringify([...song.takesOpen])); } catch (e) { /* fine */ }
  renderHeads(); songDraw();
}

// what's at a point in a take lane: {group, take, src} (the take folder whose take covers the beat, or the nearest)
function takeLaneAt(p) {
  const L = rows().lanes[p.row];
  if (!L || p.lane < 0 || p.lane >= L.n) return null;
  let best = null, dist = Infinity;
  for (const [g, nums] of L.groups) {
    const k = nums[p.lane];
    if (k == null) continue;
    for (const s of song.view.takes.filter((c) => c.group === g && c.take === k)) {
      const a = s.start, b = s.start + clipBeats(s), d = p.beat < a ? a - p.beat : p.beat > b ? p.beat - b : 0;
      if (d < dist) { dist = d; best = { group: g, take: k, src: s }; }
    }
  }
  return best;
}

function drawTakeLanes(ctx, t, i, y, sx, w, h) {
  const L = rows().lanes[i], ppb = song.ppb, x = (beat) => beat * ppb - sx, v = song.view;
  ctx.font = "600 10px system-ui, sans-serif";
  for (let j = 0; j < L.n; j++) {
    const ly = y + ROW + j * TAKE_ROW;
    if (ly > h || ly + TAKE_ROW < 0) continue;
    ctx.fillStyle = "#1a1822"; ctx.fillRect(0, ly + TAKE_ROW - 1, w, 1);
    for (const [g, nums] of L.groups) {
      const k = nums[j];
      if (k == null) continue;
      const col = takeColor(k), comp = v.clips.filter((c) => c.group === g && c.take === k);
      for (const s of v.takes.filter((c) => c.group === g && c.take === k)) {
        const x0 = x(s.start), x1 = x(s.start + clipBeats(s));
        if (x1 < 0 || x0 > w) continue;
        ctx.fillStyle = col + "16"; ctx.fillRect(x0, ly + 2, x1 - x0, TAKE_ROW - 5);
        ctx.fillStyle = col + "70"; ctx.fillRect(x0, ly + 2, 2, TAKE_ROW - 5);
        // bright where this take is in the comp
        for (const c of comp) {
          const a = x(c.start), b = x(c.start + clipBeats(c));
          ctx.fillStyle = col + "2a"; ctx.fillRect(a, ly + 2, b - a, TAKE_ROW - 5);
        }
        takeWave(ctx, s, x0, x1, ly + 3, TAKE_ROW - 7, x,
          (beat) => comp.some((c) => beat >= c.start - 1e-6 && beat < c.start + clipBeats(c) + 1e-6), col, 0);
        ctx.fillStyle = col;
        ctx.fillText(`Take ${k}`, Math.max(x0, 0) + 6, ly + 13);
      }
    }
    const d = song.takeDrag;
    if (d && d.moved && d.row === i && d.lane === j) {
      ctx.strokeStyle = "#fff"; ctx.lineWidth = 1.5;
      const a = x(Math.min(d.a, d.b)), b = x(Math.max(d.a, d.b));
      ctx.strokeRect(a, ly + 2, b - a, TAKE_ROW - 5);
    }
  }
}

function takeLaneDown(e, p) {
  const hit = takeLaneAt(p);
  if (!hit) return;
  const lanes = $("lanes");
  const g = takeGroup(hit.group), at = (ev) => Math.max(g.a, Math.min(g.b, snapBeat(lanePoint(ev).beat, snapFor(ev))));
  const d = (song.takeDrag = { row: p.row, lane: p.lane, a: at(e), b: at(e), x0: e.clientX, moved: false });
  const move = (ev) => {
    if (Math.abs(ev.clientX - d.x0) > 3) d.moved = true;
    d.b = at(ev);
    songDraw();
  };
  const up = (ev) => {
    lanes.removeEventListener("pointermove", move); lanes.removeEventListener("pointerup", up);
    song.takeDrag = null;
    if (d.moved) { compSet(hit.group, hit.take, Math.min(d.a, d.b), Math.max(d.a, d.b)); return; }
    // a click: this take for the part of the comp under the mouse (in a gap: where the take is)
    const beat = lanePoint(ev).beat, gg = takeGroup(hit.group);
    const seg = gg.comp.find((c) => beat >= c.start && beat < c.start + clipBeats(c));
    if (seg) compSet(hit.group, hit.take, seg.start, seg.start + clipBeats(seg));
    else if (beat >= hit.src.start && beat < hit.src.start + clipBeats(hit.src)) compSet(hit.group, hit.take, hit.src.start, hit.src.start + clipBeats(hit.src));
    songDraw();
  };
  lanes.addEventListener("pointermove", move); lanes.addEventListener("pointerup", up);
}

function takeLaneContext(e, p) {
  const hit = takeLaneAt(p);
  if (!hit) return;
  const g = takeGroup(hit.group);
  showCtx([
    [`Take ${hit.take} all the way`, "", () => compSet(hit.group, hit.take, g.a, g.b)],
    [`Delete take ${hit.take}`, "", async () => setSongView(await api.daw_take_op(hit.group, "delete", hit.take))],
    ["Flatten takes", "", async () => setSongView(await api.daw_take_op(hit.group, "flatten"))],
    ["Close the takes", "", () => toggleTakes(song.view.tracks[p.row].id)],
  ], e.clientX, e.clientY);
}
