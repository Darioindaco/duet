"use strict";
// Duet: the browser, like FL Studio's: your sounds on the left. Folders on
// disk (Duet's own sounds, My samples, folders you add) and bins (your own
// collections, as in Premiere or Avid, that can hold other bins). Click a
// sound to hear it; drag it into the playlist (a clip), onto a channel or a
// drum pad, into the channel rack (a new sampler channel), onto a bin or a
// folder of yours. It all lives in library.py on this computer; a sound that
// goes into the song is copied into the project and reaches your partner.
//
// Loaded before the main script; boot() calls browserSetup().

const br = {
  roots: [], bins: [], recent: [], open: new Set(), lists: {}, binLists: {},
  sel: null, query: "", results: null, info: {}, drag: null,
};

function browserSetup() {
  br.open = new Set(store.get("brOpen", []));
  $("brSearch").oninput = () => {
    clearTimeout(br.searchT);
    br.searchT = setTimeout(async () => {
      br.query = $("brSearch").value.trim();
      br.results = br.query ? await api.lib_search(br.query) : null;
      brRender();
    }, 180);
  };
  $("brImport").onclick = async () => {
    const dest = br.sel && br.sel.dir ? br.sel.path : null;
    const res = await api.lib_import(dest);
    if (res && res.files) { toast(`${res.files.length} sound${res.files.length === 1 ? "" : "s"} copied in.`); brRefresh(true); }
  };
  $("brAddFolder").onclick = async () => {
    const res = await api.lib_add_folder();
    if (res && res.error) toast(res.error, true);
    else if (res) { br.open.add(res.path); brRefresh(true); }
  };
  $("brNewBin").onclick = () => brNewBin(null);
  $("brTree").addEventListener("contextmenu", (e) => e.preventDefault());
  setInterval(brPreviewTick, 120);
  brRefresh();
}

async function brRefresh(lists = false) {
  const r = await api.lib_roots();
  Object.assign(br, r);
  if (lists) { br.lists = {}; br.binLists = {}; }
  for (const p of br.open) if (!br.lists[p] && !p.startsWith("bin:")) br.lists[p] = await api.lib_list(p);
  for (const p of br.open) if (p.startsWith("bin:") && !br.binLists[p.slice(4)]) br.binLists[p.slice(4)] = await api.lib_bin(p.slice(4));
  brRender();
}

function brToggle(key) {
  if (br.open.has(key)) br.open.delete(key); else br.open.add(key);
  store.set("brOpen", [...br.open]);
  brRefresh();
}

// ---------- the tree ----------
function brRender() {
  const box = $("brTree");
  box.innerHTML = "";
  if (br.results) {
    box.append(brHead(`Found ${br.results.length}${br.results.length >= 400 ? "+" : ""}`));
    for (const f of br.results) box.append(brFile(f, 0));
    if (!br.results.length) box.insertAdjacentHTML("beforeend", `<div class="brnone">No sounds called "${esc(br.query)}".</div>`);
    return;
  }
  if (br.recent.length) {
    box.append(brFolder({ key: "recent", name: "Recent", icon: "clock" }, 0));
    if (br.open.has("recent")) for (const f of br.recent.slice(0, 12)) box.append(brFile(f, 1));
  }
  box.append(brHead("Bins", "brNewBin"));
  const top = br.bins.filter((b) => !b.parent);
  if (!top.length) box.insertAdjacentHTML("beforeend", `<div class="brnone">Bins are your own collections: <b>+ Bin</b>, then drag sounds onto it.</div>`);
  for (const b of top) brBin(box, b, 0);
  box.append(brHead("Folders"));
  for (const r of br.roots) brDir(box, { path: r.path, name: r.name, root: r }, 0);
}

function brHead(text) {
  const h = document.createElement("div");
  h.className = "brhead";
  h.textContent = text;
  return h;
}

function brRow(depth, cls) {
  const row = document.createElement("div");
  row.className = "britem " + cls;
  row.style.paddingLeft = 6 + depth * 14 + "px";
  return row;
}

function brFolder({ key, name, icon }, depth) {
  const row = brRow(depth, "brfold" + (br.open.has(key) ? " open" : ""));
  row.innerHTML = `<i class="tw"></i><i class="ic ${icon || "dir"}"></i><span>${esc(name)}</span>`;
  row.onclick = () => brToggle(key);
  return row;
}

function brDir(box, d, depth) {
  const key = d.path;
  const row = brFolder({ key, name: d.name, icon: d.root ? d.root.kind : "dir" }, depth);
  row.dataset.dir = d.path;
  const list = br.lists[key];
  if (d.root?.missing) row.classList.add("missing");
  row.oncontextmenu = (e) => brDirMenu(e, d, list);
  row.addEventListener("click", () => { br.sel = { dir: true, path: d.path, writable: d.root ? d.root.writable : list?.writable }; });
  box.append(row);
  if (!br.open.has(key) || !list) return;
  if (list.error) { box.insertAdjacentHTML("beforeend", `<div class="brnone" style="padding-left:${20 + depth * 14}px">${esc(list.error)}</div>`); return; }
  for (const f of list.folders) brDir(box, f, depth + 1);
  for (const f of list.files) box.append(brFile(f, depth + 1));
  if (!list.folders.length && !list.files.length)
    box.insertAdjacentHTML("beforeend", `<div class="brnone" style="padding-left:${20 + depth * 14}px">Empty${list.writable ? ": drag sounds here, or Import" : ""}</div>`);
}

function brBin(box, b, depth) {
  const key = "bin:" + b.id;
  const row = brFolder({ key, name: `${b.name}  ·  ${b.count}`, icon: "bin" }, depth);
  row.dataset.bin = b.id;
  row.oncontextmenu = (e) => brBinMenu(e, b);
  box.append(row);
  if (!br.open.has(key)) return;
  for (const kid of br.bins.filter((x) => x.parent === b.id)) brBin(box, kid, depth + 1);
  const list = br.binLists[b.id];
  for (const f of list ? list.files : []) box.append(brFile(f, depth + 1, b));
  if (list && !list.files.length && !br.bins.some((x) => x.parent === b.id))
    box.insertAdjacentHTML("beforeend", `<div class="brnone" style="padding-left:${20 + depth * 14}px">Empty: drag sounds onto the bin</div>`);
}

function brFile(f, depth, bin = null) {
  const row = brRow(depth, "brfile" + (br.sel?.path === f.path && !br.sel.dir ? " sel" : "") + (f.missing ? " missing" : ""));
  row.innerHTML = `<i class="ic snd"></i><span>${esc(f.name)}</span>`;
  row.title = f.path;
  row.dataset.path = f.path;
  row.onpointerdown = (e) => { if (e.button === 0) brDragStart(e, f); };
  row.oncontextmenu = (e) => brFileMenu(e, f, bin);
  return row;
}

// ---------- choosing and hearing a sound ----------
async function brSelect(f) {
  br.sel = { path: f.path, name: f.name };
  for (const el of $("brTree").querySelectorAll(".brfile")) el.classList.toggle("sel", el.dataset.path === f.path);
  const res = await api.lib_preview(f.path);
  if (res && res.error) toast(res.error, true);
  if (!br.info[f.path]) br.info[f.path] = await api.lib_info(f.path);
  brPaintSel();
}

function brPaintSel() {
  const s = br.sel, box = $("brSel");
  if (!s || s.dir) { box.hidden = true; return; }
  box.hidden = false;
  const info = br.info[s.path] || {};
  $("brSelName").textContent = s.name;
  $("brSelMeta").textContent = info.error ? info.error
    : info.frames ? `${(info.frames / info.sr).toFixed(2)} s · ${info.channels === 2 ? "stereo" : "mono"} · ${(info.sr / 1000).toFixed(1)} kHz` : "";
  brWave();
}

function brWave(progress = null) {
  const cv = $("brWave"), s = br.sel;
  const w = cv.clientWidth || 280, h = cv.clientHeight || 54;
  const ctx = sizeCanvas(cv, w, h);
  ctx.fillStyle = "#0e0d12"; ctx.fillRect(0, 0, w, h);
  const info = s && br.info[s.path];
  if (!info || !info.peaks) return;
  if (!info.pk) info.pk = decodePeaks(info.peaks);
  const pk = info.pk, n = pk.length / 2;
  ctx.fillStyle = song.colors.me;
  for (let x = 0; x < w; x++) {
    const i0 = Math.floor(x / w * n), i1 = Math.max(i0 + 1, Math.floor((x + 1) / w * n));
    let lo = 0, hi = 0;
    for (let i = i0; i < i1; i++) { lo = Math.min(lo, pk[2 * i]); hi = Math.max(hi, pk[2 * i + 1]); }
    const top = h / 2 - hi / 127 * (h / 2 - 2), bot = h / 2 - lo / 127 * (h / 2 - 2);
    ctx.fillRect(x, top, 1, Math.max(1, bot - top));
  }
  if (progress != null) { ctx.fillStyle = "#fff"; ctx.fillRect(Math.round(progress * w), 0, 1, h); }
}

async function brPreviewTick() {
  if (!br.sel || br.sel.dir || $("brSel").hidden || !ui.sbOpen) return;
  const [pos, len] = await api.lib_preview_state();
  if (len || br.wasPlaying) brWave(len ? pos / len : null);
  br.wasPlaying = !!len;
}

// ---------- dragging a sound somewhere ----------
function brDragStart(e, f) {
  const x0 = e.clientX, y0 = e.clientY;
  let ghost = null, over = null;
  const where = (ev) => {
    if (ghost) ghost.style.display = "none";
    const el = document.elementFromPoint(ev.clientX, ev.clientY);
    if (ghost) ghost.style.display = "";
    return el;
  };
  const move = (ev) => {
    if (!ghost) {
      if (Math.hypot(ev.clientX - x0, ev.clientY - y0) < 5) return;
      ghost = document.createElement("div");
      ghost.className = "brghost";
      ghost.textContent = "♪ " + f.name;
      document.body.append(ghost);
    }
    ghost.style.left = ev.clientX + 12 + "px";
    ghost.style.top = ev.clientY + 10 + "px";
    const t = brTarget(where(ev), ev);
    if (over && over !== t?.el) over.classList.remove("brdrop");
    over = t?.el || null;
    if (over) over.classList.add("brdrop");
    ghost.classList.toggle("ok", !!t);
    ghost.dataset.what = t ? t.what : "";
    song.dropGhost = t && t.lane ? t.lane : null;
    if (winShown("song")) drawOverlay();
  };
  const up = async (ev) => {
    document.removeEventListener("pointermove", move);
    document.removeEventListener("pointerup", up);
    if (over) over.classList.remove("brdrop");
    song.dropGhost = null;
    if (!ghost) { brSelect(f); return; } // a click: hear it
    ghost.remove();
    if (winShown("song")) drawOverlay();
    const t = brTarget(where(ev), ev);
    if (t) await t.drop(f);
  };
  document.addEventListener("pointermove", move);
  document.addEventListener("pointerup", up);
}

// what's under the pointer that takes a sound: {el, what, drop(f), lane?}
function brTarget(el, ev) {
  if (!el) return null;
  const binEl = el.closest(".britem[data-bin]");
  if (binEl) return { el: binEl, what: "Add to the bin", drop: async (f) => { await api.lib_bin_add(binEl.dataset.bin, [f.path]); brRefresh(true); } };
  const dirEl = el.closest(".britem[data-dir]");
  if (dirEl) {
    const root = br.roots.find((r) => dirEl.dataset.dir === r.path || dirEl.dataset.dir.startsWith(r.path));
    if (!root || !root.writable) return null;
    return { el: dirEl, what: "Copy here", drop: (f) => brCopyTo(f, dirEl.dataset.dir) };
  }
  const pad = el.closest(".pad[data-pad]");
  if (pad) return { el: pad, what: "Onto this pad", drop: (f) => brToPad(pad.dataset.t, +pad.dataset.pad, f) };
  const smp = el.closest(".smpbox[data-t]");
  if (smp) return { el: smp, what: "The sampler's sound", drop: (f) => brToPad(smp.dataset.t, 0, f) };
  const row = el.closest("#chanRows .srow[data-key]");
  const chan = el.closest("#chanRows .chan[data-id]");
  if (chan) {
    const t = trackById(chan.dataset.id);
    if (t?.inst?.kind === "drums" && row) {
      const pad = DRUM_PADS.indexOf(+row.dataset.key);
      if (pad >= 0) return { el: row, what: "Onto this pad", drop: (f) => brToPad("T" + t.slot, pad, f) };
    }
    if (t?.inst?.kind === "sampler") return { el: chan, what: "The sampler's sound", drop: (f) => brToPad("T" + t.slot, 0, f) };
  }
  if (el.closest("#chanRows")) return { el: $("chanRows"), what: "A new channel", drop: brToChannel };
  if (el.closest("#lanes")) {
    const p = lanePoint(ev);
    const at = song.view.tracks[p.row], tr = at && !isAuto(at) ? at : null;
    const beat = Math.max(0, snapBeat(p.beat));
    return { el: $("lanes"), what: tr ? "Onto " + tr.name : "A new track", lane: { beat, row: tr ? p.row : song.view.tracks.length },
      drop: (f) => brToSong(f, tr ? tr.id : null, beat) };
  }
  return null;
}

// drum kit pads, in the order of the kit (GM 36..51)
const DRUM_PADS = Array.from({ length: 16 }, (_, i) => 36 + i);

async function brToSong(f, track, beat) {
  // a big file takes a moment to copy into the project: say so
  const slow = setTimeout(() => toast(`Adding ${f.name}…`), 400);
  const res = await api.lib_to_song(f.path, track, beat);
  clearTimeout(slow);
  if (res && res.error) toast(res.error, true);
  else setSongView(await api.daw_project());
}

async function brToChannel(f) {
  const res = await api.lib_to_channel(f.path);
  if (res && res.error) { toast(res.error, true); return; }
  setSongView(await api.daw_project());
  if (res.track) selectChannel(res.track);
  toast(`"${f.name}" is a channel: click steps, or draw notes in the piano roll (C4 plays it as it is).`);
}

async function brToPad(target, pad, f) {
  const res = await api.inst_pad(target, pad, f.path);
  if (res && res.error) toast(res.error, true);
  else { await rackChanged(res); if (target[0] === "T") setSongView(await api.daw_project()); }
}

async function brCopyTo(f, dir) {
  const res = await api.lib_copy(f.path, dir);
  if (res && res.error) toast(res.error, true);
  brRefresh(true);
}

// ---------- menus ----------
function brFileMenu(e, f, bin) {
  e.preventDefault();
  const items = [
    ["Hear it", "click", () => brSelect(f)],
    ["Into the playlist (a new track)", "", () => brToSong(f, null, Math.max(0, Math.floor(curPos() / bpb()) * bpb()))],
    ["As a new channel (sampler)", "", () => brToChannel(f)],
  ];
  for (const b of br.bins) items.push([`Add to bin: ${b.name}`, "", async () => { await api.lib_bin_add(b.id, [f.path]); brRefresh(true); }]);
  if (bin) items.push([`Take out of "${bin.name}"`, "", async () => { await api.lib_bin_remove(bin.id, f.path); brRefresh(true); }]);
  const root = br.roots.find((r) => f.path.startsWith(r.path));
  if (!bin && root && root.writable) items.push(["Rename…", "", () => brRename(f.path, f.name)]);
  showCtx(items, e.clientX, e.clientY);
}

function brDirMenu(e, d, list) {
  e.preventDefault();
  const writable = d.root ? d.root.writable : list?.writable;
  const items = [];
  if (writable) items.push(["New folder here…", "", async () => {
    const name = prompt("Folder name", "New folder");
    if (!name) return;
    const res = await api.lib_new_folder(d.path, name);
    if (res.error) toast(res.error, true); else { br.open.add(d.path); brRefresh(true); }
  }], ["Import sounds here…", "", async () => { const r = await api.lib_import(d.path); if (r) brRefresh(true); }]);
  if (writable && !d.root) items.push(["Rename…", "", () => brRename(d.path, d.name)]);
  if (d.root && d.root.kind === "folder") items.push(["Remove from the browser", "", async () => { await api.lib_remove_folder(d.path); brRefresh(true); }]);
  if (!items.length) items.push(["Duet's own sounds (they stay as they are)", "", () => {}, true]);
  showCtx(items, e.clientX, e.clientY);
}

function brBinMenu(e, b) {
  e.preventDefault();
  showCtx([
    ["New bin inside…", "", () => brNewBin(b.id)],
    ["Rename…", "", async () => { const n = prompt("Bin name", b.name); if (n) { await api.lib_rename_bin(b.id, n); brRefresh(true); } }],
    ...(b.parent ? [["Move to the top", "", async () => { await api.lib_move_bin(b.id, null); brRefresh(true); }]] : []),
    ["Delete the bin (the sounds stay)", "", async () => { if (confirm(`Delete the bin "${b.name}" and the bins in it? The sounds themselves stay where they are.`)) { await api.lib_delete_bin(b.id); brRefresh(true); } }],
  ], e.clientX, e.clientY);
}

async function brNewBin(parent) {
  const name = prompt("Bin name", "New bin");
  if (!name) return;
  const res = await api.lib_new_bin(name, parent);
  if (parent) br.open.add("bin:" + parent);
  if (res.id) br.open.add("bin:" + res.id);
  brRefresh(true);
}

async function brRename(path, name) {
  const n = prompt("New name", name);
  if (!n || n === name) return;
  const res = await api.lib_rename(path, n);
  if (res.error) toast(res.error, true); else brRefresh(true);
}
