"use strict";
// Duet: the workspace, like FL Studio's. Every panel is a window on the desk:
// drag it by its title bar, size it from any edge or corner. Edges stick to the
// desk's edges and to the other windows. Drag a window against an edge of the
// desk to fill that half (a corner: that quarter; the top: all of it), or
// double-click its title. Positions are kept as parts of the desk, so a layout
// follows the size of the app's window; layouts can be saved as workspaces.
//
// Loaded before the main script; boot() calls wmSetup() before anything draws.

const WINS = ["song", "rack", "roll", "mixer", "fx"];
const WM_MIN = { w: 260, h: 130 };
const WM_STICK = 10;  // px: edges closer than this stick together
const WM_EDGE = 8;    // px from the desk's edge: let go to fill a half or a quarter

// built-in layouts: [x, y, w, h] as parts of the desk; a window left out is closed
const LAYOUTS = {
  Compose: { song: [0, 0, 1, 0.56], rack: [0, 0.56, 0.42, 0.44], roll: [0.42, 0.56, 0.58, 0.44] },
  Arrange: { song: [0, 0, 1, 1] },
  Mix: { song: [0, 0, 1, 0.42], mixer: [0, 0.42, 0.6, 0.58], fx: [0.6, 0.42, 0.4, 0.58] },
  "Mixer on the right": { song: [0, 0, 0.62, 0.56], rack: [0, 0.56, 0.62, 0.44], mixer: [0.62, 0, 0.38, 0.62], fx: [0.62, 0.62, 0.38, 0.38] },
  "Piano roll big": { song: [0, 0, 1, 0.3], roll: [0, 0.3, 1, 0.7] },
};
// where a window opens when the layout doesn't place it
const WM_HOME = { song: [0, 0, 1, 0.56], rack: [0.04, 0.3, 0.5, 0.42], roll: [0.3, 0.32, 0.66, 0.6], mixer: [0.2, 0.2, 0.75, 0.55], fx: [0.25, 0.45, 0.72, 0.5] };

const wm = { st: {}, z: 10, focus: null };

function wmSetup() {
  for (const el of document.querySelectorAll(".win")) {
    const k = el.dataset.w;
    const body = document.createElement("div");
    body.className = "wbody";
    body.append(...el.childNodes);
    const bar = document.createElement("div");
    bar.className = "wbar";
    bar.innerHTML = `<span class="wt"${el.dataset.titleId ? ` id="${el.dataset.titleId}"` : ""}>${esc(el.dataset.title)}</span><span class="spacer"></span>
      <button class="wb max" title="Fill the workspace / back (double-click the title)" aria-label="Maximize"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" aria-hidden="true"><rect x="4" y="4" width="16" height="16" rx="2"/></svg></button>
      <button class="wb x" title="Close" aria-label="Close"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"/></svg></button>`;
    el.append(bar, body);
    for (const d of ["n", "s", "e", "w", "ne", "nw", "se", "sw"]) {
      const h = document.createElement("div");
      h.className = "wr " + d;
      h.onpointerdown = (e) => wmResize(e, k, d);
      el.append(h);
    }
    bar.onpointerdown = (e) => { if (!e.target.closest("button")) wmDrag(e, k); };
    bar.ondblclick = (e) => { if (!e.target.closest("button")) wmMax(k); };
    bar.querySelector(".max").onclick = () => wmMax(k);
    bar.querySelector(".x").onclick = () => wmClose(k);
    el.addEventListener("pointerdown", () => wmFocus(k), true);
  }
  wmLoad(store.get("wm", null) || wmFromLayout(LAYOUTS.Compose));
  for (const b of $("viewSeg").querySelectorAll("button[data-w]")) b.onclick = () => wmToggle(b.dataset.w);
  $("wsBtn").onclick = wsMenu;
  new ResizeObserver(() => WINS.forEach(wmPlace)).observe($("desk"));
  // FL's keys for the windows
  document.addEventListener("keydown", (e) => {
    const k = { F5: "song", F6: "rack", F7: "roll", F9: "mixer" }[e.key];
    if (k && !e.ctrlKey && !e.altKey) { e.preventDefault(); wmToggle(k); }
  });
  // clicking a strip's empty space in the mixer shows its effects
  $("strips").addEventListener("click", (e) => {
    if (e.target.closest(".strip") && !e.target.closest("button, select, input, .knob, .fader, .fdr, .hs")) wmOpen("fx", false);
  });
}

function wmFromLayout(lay) {
  const st = {};
  WINS.forEach((k, i) => {
    const r = lay[k] || WM_HOME[k];
    st[k] = { x: r[0], y: r[1], w: r[2], h: r[3], open: !!lay[k], max: false, z: i + 1 };
  });
  st.focus = Object.keys(lay)[0];
  return st;
}

function wmLoad(st) {
  for (const k of WINS) wm.st[k] = { ...wmFromLayout(LAYOUTS.Compose)[k], ...(st[k] || {}) };
  wm.z = Math.max(10, ...WINS.map((k) => wm.st[k].z || 0));
  for (const k of WINS) wmPlace(k);
  const f = st.focus && wm.st[st.focus]?.open ? st.focus : WINS.find((k) => wm.st[k].open) || "song";
  if (!wm.st[f].open) wm.st[f].open = true;
  wmPlace(f);
  wmFocus(f);
  wmPaintButtons();
}

function wmSave() {
  store.set("wm", { ...wm.st, focus: wm.focus });
  wmPaintButtons();
}

const winShown = (k) => !!wm.st[k]?.open;
const deskRect = () => $("desk").getBoundingClientRect();

// fractions → pixels on the desk
function wmPlace(k) {
  const el = $("win-" + k), s = wm.st[k];
  if (!el || !s) return;
  el.hidden = !s.open;
  el.classList.toggle("max", !!s.max);
  el.style.zIndex = s.z;
  if (s.max) { el.style.left = el.style.top = "0px"; el.style.width = el.style.height = "100%"; return; }
  const d = deskRect();
  const w = Math.max(WM_MIN.w, s.w * d.width), h = Math.max(WM_MIN.h, s.h * d.height);
  el.style.width = Math.min(w, d.width) + "px";
  el.style.height = Math.min(h, d.height) + "px";
  el.style.left = Math.max(0, Math.min(d.width - Math.min(w, d.width), s.x * d.width)) + "px";
  el.style.top = Math.max(0, Math.min(d.height - Math.min(h, d.height), s.y * d.height)) + "px";
}

function wmSetRect(k, r) {
  const d = deskRect();
  Object.assign(wm.st[k], { x: r.x / d.width, y: r.y / d.height, w: r.w / d.width, h: r.h / d.height, max: false });
  wmPlace(k);
}

function wmFocus(k) {
  if (wm.focus === k && +$("win-" + k).style.zIndex === wm.z) return;
  wm.focus = k;
  wm.st[k].z = ++wm.z;
  $("win-" + k).style.zIndex = wm.z;
  for (const el of document.querySelectorAll(".win")) el.classList.toggle("focus", el.dataset.w === k);
  // the rest of the app thinks in three places: the playlist, the sequencer, the mixer
  ui.view = { song: "song", rack: "seq", roll: "seq", mixer: "mixer", fx: "mixer" }[k];
  if (typeof syncMidiTarget === "function") syncMidiTarget();
  wmSave();
}

function wmOpen(k, focus = true) {
  const s = wm.st[k];
  const was = s.open;
  s.open = true;
  wmPlace(k);
  if (focus || !was) wmFocus(k);
  if (!was) wmShown(k);
  wmSave();
}

// draw what a window holds, now that it can be seen
function wmShown(k) {
  if (typeof song === "undefined" || !song.view) return;
  if (k === "song") requestAnimationFrame(songDraw);
  if (k === "rack" || k === "roll") requestAnimationFrame(seqRender);
}

function wmClose(k) {
  wm.st[k].open = false;
  wm.st[k].max = false;
  wmPlace(k);
  if (wm.focus === k) {
    const next = WINS.filter((w) => wm.st[w].open).sort((a, b) => wm.st[b].z - wm.st[a].z)[0];
    if (next) wmFocus(next);
  }
  wmSave();
}

// the header buttons: open it, bring it to the front, or (already in front) close it
function wmToggle(k) {
  if (!wm.st[k].open) wmOpen(k);
  else if (wm.focus !== k) wmFocus(k);
  else wmClose(k);
}

function wmMax(k) {
  wm.st[k].max = !wm.st[k].max;
  wmPlace(k);
  wmFocus(k);
  wmSave();
}

function wmPaintButtons() {
  for (const b of $("viewSeg").querySelectorAll("button[data-w]")) {
    b.classList.toggle("on", winShown(b.dataset.w));
    b.classList.toggle("front", wm.focus === b.dataset.w);
  }
}

// ---------- moving and sizing ----------
// the edges a moving edge can stick to: the desk's and the other open windows'
function wmTargets(k) {
  const d = deskRect(), xs = [0, d.width], ys = [0, d.height];
  for (const w of WINS) {
    if (w === k || !wm.st[w].open || wm.st[w].max) continue;
    const el = $("win-" + w);
    const l = el.offsetLeft, t = el.offsetTop;
    xs.push(l, l + el.offsetWidth);
    ys.push(t, t + el.offsetHeight);
  }
  return { xs, ys };
}
const stickTo = (v, list) => { let best = null; for (const t of list) if (Math.abs(t - v) <= WM_STICK && (best == null || Math.abs(t - v) < Math.abs(best - v))) best = t; return best; };

// let go against the desk's edge: fill a half, a quarter, or all of it
function wmZone(px, py, d) {
  const l = px <= WM_EDGE, r = px >= d.width - WM_EDGE, t = py <= WM_EDGE, b = py >= d.height - WM_EDGE;
  const W = d.width, H = d.height;
  if (t && !l && !r) return { max: true, x: 0, y: 0, w: W, h: H };
  if (l && t) return { x: 0, y: 0, w: W / 2, h: H / 2 };
  if (r && t) return { x: W / 2, y: 0, w: W / 2, h: H / 2 };
  if (l && b) return { x: 0, y: H / 2, w: W / 2, h: H / 2 };
  if (r && b) return { x: W / 2, y: H / 2, w: W / 2, h: H / 2 };
  if (l) return { x: 0, y: 0, w: W / 2, h: H };
  if (r) return { x: W / 2, y: 0, w: W / 2, h: H };
  if (b) return { x: 0, y: H / 2, w: W, h: H / 2 };
  return null;
}

function wmGhost(z) {
  const g = $("snapGhost");
  g.hidden = !z;
  if (z) Object.assign(g.style, { left: z.x + "px", top: z.y + "px", width: z.w + "px", height: z.h + "px" });
}

function wmDrag(e, k) {
  e.preventDefault();
  const el = $("win-" + k), bar = e.currentTarget, d = deskRect();
  wmFocus(k);
  // a maximized window comes loose at its old size, under the pointer
  if (wm.st[k].max) {
    const s = wm.st[k];
    s.max = false;
    const w = s.w * d.width;
    s.x = Math.max(0, (e.clientX - d.left - w / 2)) / d.width;
    s.y = 0;
    wmPlace(k);
  }
  const ox = e.clientX - d.left - el.offsetLeft, oy = e.clientY - d.top - el.offsetTop;
  const w = el.offsetWidth, h = el.offsetHeight;
  const { xs, ys } = wmTargets(k);
  bar.setPointerCapture(e.pointerId);
  el.classList.add("moving");
  let zone = null, rect = { x: el.offsetLeft, y: el.offsetTop, w, h };
  const move = (ev) => {
    const px = ev.clientX - d.left, py = ev.clientY - d.top;
    zone = wmZone(px, py, d);
    wmGhost(zone);
    let x = px - ox, y = py - oy;
    // stick: either edge of the window to a target
    const sx = stickTo(x, xs), sx2 = stickTo(x + w, xs);
    if (sx != null) x = sx; else if (sx2 != null) x = sx2 - w;
    const sy = stickTo(y, ys), sy2 = stickTo(y + h, ys);
    if (sy != null) y = sy; else if (sy2 != null) y = sy2 - h;
    x = Math.max(0, Math.min(d.width - w, x));
    y = Math.max(0, Math.min(d.height - h, y));
    rect = { x, y, w, h };
    el.style.left = x + "px"; el.style.top = y + "px";
  };
  const up = () => {
    bar.removeEventListener("pointermove", move);
    bar.removeEventListener("pointerup", up);
    el.classList.remove("moving");
    wmGhost(null);
    if (zone && zone.max) wmMax(k);
    else wmSetRect(k, zone || rect);
    wmSave();
  };
  bar.addEventListener("pointermove", move);
  bar.addEventListener("pointerup", up);
}

function wmResize(e, k, dir) {
  e.preventDefault();
  e.stopPropagation();
  const el = $("win-" + k), h0 = e.currentTarget, d = deskRect();
  wmFocus(k);
  const r0 = { l: el.offsetLeft, t: el.offsetTop, r: el.offsetLeft + el.offsetWidth, b: el.offsetTop + el.offsetHeight };
  const x0 = e.clientX, y0 = e.clientY;
  const { xs, ys } = wmTargets(k);
  h0.setPointerCapture(e.pointerId);
  el.classList.add("moving");
  let r = { ...r0 };
  const move = (ev) => {
    const dx = ev.clientX - x0, dy = ev.clientY - y0;
    r = { ...r0 };
    const edge = (v, list) => stickTo(v, list) ?? v;
    if (dir.includes("w")) r.l = Math.max(0, Math.min(r0.r - WM_MIN.w, edge(r0.l + dx, xs)));
    if (dir.includes("e")) r.r = Math.min(d.width, Math.max(r0.l + WM_MIN.w, edge(r0.r + dx, xs)));
    if (dir.includes("n")) r.t = Math.max(0, Math.min(r0.b - WM_MIN.h, edge(r0.t + dy, ys)));
    if (dir.includes("s")) r.b = Math.min(d.height, Math.max(r0.t + WM_MIN.h, edge(r0.b + dy, ys)));
    Object.assign(el.style, { left: r.l + "px", top: r.t + "px", width: r.r - r.l + "px", height: r.b - r.t + "px" });
  };
  const up = () => {
    h0.removeEventListener("pointermove", move);
    h0.removeEventListener("pointerup", up);
    el.classList.remove("moving");
    wmSetRect(k, { x: r.l, y: r.t, w: r.r - r.l, h: r.b - r.t });
    wmSave();
  };
  h0.addEventListener("pointermove", move);
  h0.addEventListener("pointerup", up);
}

// ---------- workspaces: the built-in layouts and your own ----------
function wsMenu() {
  const saved = store.get("wmSaved", {});
  const r = $("wsBtn").getBoundingClientRect();
  const items = Object.keys(LAYOUTS).map((n) => ["Layout: " + n, "", () => wsApply(wmFromLayout(LAYOUTS[n]))]);
  for (const n of Object.keys(saved)) items.push(["★ " + n, "", () => wsApply(saved[n])]);
  items.push(["Save this layout…", "", () => {
    const n = prompt("Name for this workspace", "My layout");
    if (!n || !n.trim()) return;
    store.set("wmSaved", { ...store.get("wmSaved", {}), [n.trim()]: { ...JSON.parse(JSON.stringify(wm.st)), focus: wm.focus } });
    toast(`Saved "${n.trim()}". It's in this menu now.`);
  }]);
  for (const n of Object.keys(saved)) items.push([`Forget "${n}"`, "", () => {
    const s = store.get("wmSaved", {});
    delete s[n];
    store.set("wmSaved", s);
  }]);
  showCtx(items, r.left, r.bottom + 4);
}

function wsApply(st) {
  const before = WINS.filter(winShown);
  wmLoad(JSON.parse(JSON.stringify(st)));
  for (const k of WINS) if (winShown(k) && !before.includes(k)) wmShown(k);
  wmSave();
}
