// Keyboard shortcuts you can set yourself, with sets in the style of other
// DAWs and editors (FL Studio, Logic, Pro Tools, Premiere/Avid). Saved on this
// computer only: your partner keeps their own.
//
// A shortcut is written "Mod+Shift+K": Mod is Ctrl (Cmd on a Mac), letters
// by the key's place on the keyboard, so they work in any layout.

const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

// [id, label, group, the Duet set's keys, what it does]
const ACTIONS = [
  ["play", "Play / stop", "Transport", ["Space"], () => togglePlay()],
  ["record", "Record", "Transport", ["R"], () => toggleRec()],
  ["home", "Go to the start", "Transport", ["Home"], () => { api.daw_locate(0); if (song.st && !song.st.playing) song.st.pos = 0; }],
  ["nextEdge", "Playhead to the next clip edge", "Transport", ["Tab"], () => jumpEdge(1)],
  ["prevEdge", "Playhead to the previous clip edge", "Transport", ["Shift+Tab"], () => jumpEdge(-1)],
  ["setIn", "Set the in point (loop start) here", "In & out", ["I"], () => setInOut("in")],
  ["setOut", "Set the out point (loop end) here", "In & out", ["O"], () => setInOut("out")],
  ["loopSel", "In and out around the selected clips", "In & out", ["Mod+U"], () => loopToSelection()],
  ["clearInOut", "Clear in and out (loop off)", "In & out", ["Alt+X"], () => songEdit([["meta", "meta", { loop: false }]])],
  ["split", "Split at the playhead", "Editing", ["S"], () => splitAtPlayhead()],
  ["crossfade", "Crossfade the selected clips where they meet", "Editing", ["X"], () => crossfadeSel()],
  ["fadeIn", "Fade in up to the playhead", "Editing", ["Alt+I"], () => fadeToPlayhead("in")],
  ["fadeOut", "Fade out from the playhead", "Editing", ["Alt+O"], () => fadeToPlayhead("out")],
  ["trimStart", "Trim the clip's start to the playhead", "Editing", ["Alt+BracketLeft"], () => trimToPlayhead("start")],
  ["trimEnd", "Trim the clip's end to the playhead", "Editing", ["Alt+BracketRight"], () => trimToPlayhead("end")],
  ["nudgeLeft", "Nudge left (one grid step)", "Editing", ["Alt+Left"], () => nudgeSel(-1)],
  ["nudgeRight", "Nudge right (one grid step)", "Editing", ["Alt+Right"], () => nudgeSel(1)],
  ["reverse", "Reverse the selected clips", "Editing", ["Shift+R"], () => songEdit(selClips().filter((c) => !c.pattern).map((c) => ["clip", c.id, { reverse: !c.reverse }]))],
  ["editClip", "Open the clip editor", "Editing", ["E"], () => editSelClip()],
  ["mute", "Mute / unmute the selected clips", "Editing", ["Alt+M"], () => muteSel()],
  ["duplicate", "Duplicate", "Editing", ["Mod+D"], () => duplicateSel()],
  ["copy", "Copy", "Editing", ["Mod+C"], () => copySel()],
  ["cut", "Cut", "Editing", ["Mod+X"], () => copySel(true)],
  ["paste", "Paste at the playhead", "Editing", ["Mod+V"], () => paste()],
  ["delete", "Delete", "Editing", ["Delete", "Backspace"], () => song.sel.size && deleteSel()],
  ["selectAll", "Select everything", "Selection", ["Mod+A"], () => { song.view.clips.forEach((c) => song.sel.add(c.id)); songDraw(); }],
  ["deselect", "Select nothing", "Selection", ["Escape"], () => { hideCtx(); song.sel.clear(); songDraw(); }],
  ["undo", "Undo", "Song", ["Mod+Z"], () => undo()],
  ["redo", "Redo", "Song", ["Mod+Shift+Z", "Mod+Y"], () => redo()],
  ["export", "Export", "Song", ["Mod+E"], () => openExport()],
  ["zoomIn", "Zoom in", "View", ["Equal"], () => zoomBy(1.25)],
  ["zoomOut", "Zoom out", "View", ["Minus"], () => zoomBy(0.8)],
  ["zoomSel", "Zoom to the selection", "View", ["Z"], () => zoomToSelection()],
  ["shortcuts", "These shortcuts", "View", ["Mod+Slash"], () => openShortcuts()],
];
// work in every view (the rest only in the playlist)
const GLOBAL_ACTIONS = new Set(["play", "record", "home", "export", "shortcuts"]);

// the other sets: only what differs from Duet's
const KEY_PRESETS = {
  duet: { name: "Duet", map: {} },
  fl: { name: "FL Studio", map: { duplicate: ["Mod+B"], export: ["Mod+R"], record: ["R"], split: ["Mod+T"], zoomSel: ["Z"] } },
  logic: { name: "Logic", map: { split: ["Mod+T"], loopSel: ["Mod+U"], record: ["R"], home: ["Enter", "Home"], zoomSel: ["Z"], editClip: ["E"],
    crossfade: ["Mod+Alt+X"] } },
  protools: { name: "Pro Tools", map: { split: ["B", "Mod+E"], trimStart: ["A"], trimEnd: ["S"], fadeIn: ["D"], fadeOut: ["G"], crossfade: ["F"],
    export: ["Mod+Alt+B"], record: ["Mod+Space"], editClip: ["Mod+Shift+E"], nudgeLeft: ["Comma"], nudgeRight: ["Period"] } },
  premiere: { name: "Premiere / Avid", map: { split: ["Mod+K"], crossfade: ["Mod+Shift+D"], setIn: ["I"], setOut: ["O"], clearInOut: ["Mod+Shift+X"],
    nudgeLeft: ["Alt+Left"], nudgeRight: ["Alt+Right"], zoomSel: ["Backslash"] } },
};

// (what was saved is read in keysSetup: `store` comes later in the page)
const keyCfg = { preset: "duet", custom: {}, xfadeMs: 50 };

function keysFor(id) {
  if (keyCfg.custom[id]) return keyCfg.custom[id];
  const p = KEY_PRESETS[keyCfg.preset] || KEY_PRESETS.duet;
  return p.map[id] || ACTIONS.find((a) => a[0] === id)[3];
}

// the key pressed, as a shortcut ("Mod+Shift+K"); null for a lone modifier
function comboOf(e) {
  const code = e.code || "";
  if (/^(Shift|Control|Alt|Meta|OS)/.test(code) || ["Shift", "Control", "Alt", "Meta"].includes(e.key)) return null;
  let k = code.startsWith("Key") ? code.slice(3) : code.startsWith("Digit") ? code.slice(5)
    : code.startsWith("Arrow") ? code.slice(5) : code.startsWith("Numpad") ? code : code || e.key;
  if (k === " ") k = "Space";
  const mods = [];
  if (e.ctrlKey || e.metaKey) mods.push("Mod");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  return [...mods, k].join("+");
}

// how a shortcut reads on this computer
function comboLabel(c) {
  const names = { Mod: IS_MAC ? "⌘" : "Ctrl", Alt: IS_MAC ? "⌥" : "Alt", Shift: IS_MAC ? "⇧" : "Shift", BracketLeft: "[", BracketRight: "]",
    Equal: "=", Minus: "−", Slash: "/", Backslash: "\\", Comma: ",", Period: ".", Left: "←", Right: "→", Up: "↑", Down: "↓", Escape: "Esc", Backspace: "⌫" };
  return c.split("+").map((p) => names[p] || p).join(IS_MAC ? "" : "+");
}

function actionFor(combo) {
  for (const a of ACTIONS) if (keysFor(a[0]).includes(combo)) return a;
  return null;
}

// the one keydown handler for the song (the sequencer has its own keys)
function songKeys(e) {
  if (!$("keysM").hidden) return; // setting a shortcut
  if (e.target.closest("input, select, textarea") || !$("vstm").hidden || !$("newTrackM").hidden || !$("clipEd").hidden) return;
  const combo = comboOf(e);
  if (!combo) return;
  const a = actionFor(combo);
  if (!a) return;
  if (ui.view === "seq" && a[0] !== "export" && a[0] !== "shortcuts") return;
  if (!GLOBAL_ACTIONS.has(a[0]) && ui.view !== "song") return;
  if (e.target.closest("button")) e.target.blur(); // or Space would press that button too
  e.preventDefault();
  a[4]();
}

// --- what the new actions do ---------------------------------------------------------

function setInOut(which) {
  const m = song.view.meta, at = Math.max(0, snapBeat(curPos()));
  let a = which === "in" ? at : m.loop_start, b = which === "out" ? at : m.loop_end;
  if (b <= a) { if (which === "in") b = a + bpb(); else a = Math.max(0, b - bpb()); }
  songEdit([["meta", "meta", { loop: true, loop_start: a, loop_end: b }]]);
  toast(`${which === "in" ? "In" : "Out"} point at ${fmtPos(at)}`);
}

function loopToSelection() {
  const sel = selClips();
  if (!sel.length) { toast("Select some clips first."); return; }
  const a = Math.min(...sel.map((c) => c.start)), b = Math.max(...sel.map((c) => c.start + clipBeats(c)));
  songEdit([["meta", "meta", { loop: true, loop_start: a, loop_end: b }]]);
}

// clips to work on: the selected ones under the playhead, else every audio clip under it
function clipsAtPlayhead() {
  const at = curPos(), under = (c) => !c.pattern && at > c.start + 1e-6 && at < c.start + clipBeats(c) - 1e-6;
  const sel = selClips().filter(under);
  return sel.length ? sel : song.view.clips.filter(under);
}

function fadeToPlayhead(which) {
  const s = spb(), at = curPos(), ch = [];
  for (const c of clipsAtPlayhead()) {
    const t = (at - c.start) * s;
    ch.push(["clip", c.id, which === "in" ? { fade_in: Math.min(t, c.len - (c.fade_out || 0)) } : { fade_out: Math.min(c.len - t, c.len - (c.fade_in || 0)) }]);
  }
  if (ch.length) songEdit(ch); else toast("No clip under the playhead.");
}

function trimToPlayhead(edge) {
  const s = spb(), at = curPos(), ch = [];
  for (const c of clipsAtPlayhead()) {
    const t = (at - c.start) * s, cut = cutOffsets(c, t);
    ch.push(["clip", c.id, edge === "start"
      ? { start: at, offset: cut.right, len: c.len - t, fade_in: Math.min(c.fade_in || 0, c.len - t) }
      : { offset: cut.left, len: t, fade_out: Math.min(c.fade_out || 0, t) }]);
  }
  if (ch.length) songEdit(ch); else toast("No clip under the playhead.");
}

function nudgeSel(dir) {
  const sel = selClips();
  if (!sel.length) return;
  const step = gridOf(song.snap) || 0.0625;
  const d = Math.max(dir * step, -Math.min(...sel.map((c) => c.start)));
  songEdit(sel.map((c) => ["clip", c.id, { start: c.start + d }]));
}

// where two selected clips on a track meet (or overlap): they overlap by the
// crossfade length, one fading out while the other fades in
function crossfadeSel() {
  const s = spb(), xf = Math.max(0.005, (keyCfg.xfadeMs || 50) / 1000), ch = [];
  let sel = selClips().filter((c) => !c.pattern);
  if (sel.length === 1) {
    // one clip: with the one that starts where it ends (or ends where it starts)
    const c = sel[0], end = c.start + clipBeats(c), near = (a, b) => Math.abs(a - b) * s < 0.05;
    const other = song.view.clips.find((k) => k.track === c.track && k.id !== c.id && !k.pattern && (near(k.start, end) || near(k.start + clipBeats(k), c.start)));
    if (other) sel = [c, other];
  }
  // worked out on copies, so a clip between two others gets both ends right
  const work = {}, edits = {};
  const set = (c, f) => { Object.assign(work[c.id], f); Object.assign(edits[c.id] ||= {}, f); };
  const byTrack = {};
  for (const c of sel) { work[c.id] = { ...c }; (byTrack[c.track] ||= []).push(work[c.id]); }
  for (const list of Object.values(byTrack)) {
    list.sort((x, y) => x.start - y.start);
    for (let i = 0; i + 1 < list.length; i++) {
      const a = list[i], b = list[i + 1];
      const gap = (b.start - (a.start + a.len / s)) * s; // seconds (negative: they overlap)
      if (gap > 0.05) continue; // not meeting
      let ov = -gap;
      if (ov < xf) {
        // make room: the first one a little longer, the second starts a little earlier
        const ra = clipRatio(a), roomA = a.reverse ? (a.offset || 0) : fileDur(a) - (a.offset || 0) - a.len * ra;
        const rb = clipRatio(b), roomB = b.reverse ? fileDur(b) - (b.offset || 0) - b.len * rb : (b.offset || 0);
        const ga = Math.max(0, Math.min((xf - ov) / 2, roomA / ra)), gb = Math.max(0, Math.min(xf - ov - ga, roomB / rb));
        set(a, { len: a.len + ga, ...(a.reverse ? { offset: (a.offset || 0) - ga * ra } : {}) });
        set(b, { start: b.start - gb / s, len: b.len + gb, ...(b.reverse ? {} : { offset: (b.offset || 0) - gb * rb }) });
        ov += ga + gb;
      }
      if (ov < 0.002) continue;
      set(a, { fade_out: ov });
      set(b, { fade_in: ov });
    }
  }
  for (const [id, f] of Object.entries(edits)) ch.push(["clip", id, f]);
  if (ch.length) songEdit(ch);
  else toast("Select two clips on a track that meet (or one, next to another).");
}

function jumpEdge(dir) {
  const at = curPos(), m = song.view.meta, edges = new Set([0]);
  for (const c of song.view.clips) { edges.add(c.start); edges.add(c.start + clipBeats(c)); }
  if (m.loop) { edges.add(m.loop_start); edges.add(m.loop_end); }
  const list = [...edges].sort((a, b) => a - b);
  const next = dir > 0 ? list.find((b) => b > at + 1e-6) : [...list].reverse().find((b) => b < at - 1e-6);
  if (next == null) return;
  api.daw_locate(next);
  if (song.st && !song.st.playing) song.st.pos = next;
}

function zoomToSelection() {
  const sel = selClips(), lanes = $("lanes");
  const list = sel.length ? sel : song.view.clips;
  if (!list.length) return;
  const a = Math.min(...list.map((c) => c.start)), b = Math.max(...list.map((c) => c.start + clipBeats(c)));
  song.ppb = Math.max(2, Math.min(240, (lanes.clientWidth - 40) / Math.max(0.25, b - a)));
  store.set("songZoom", song.ppb);
  songRender();
  lanes.scrollLeft = Math.max(0, a * song.ppb - 20);
}

function editSelClip() {
  const c = selClips().find((k) => !k.pattern) || clipsAtPlayhead()[0];
  if (c) openClipEditor(c.id); else toast("Select an audio clip first.");
}

// --- the settings window ---------------------------------------------------------------

let keyListen = null; // the action waiting for its new key

function openShortcuts() {
  $("keysM").hidden = false;
  keysRender();
}

function keysRender() {
  const box = $("keysList");
  $("keysPreset").innerHTML = "";
  for (const [id, p] of Object.entries(KEY_PRESETS)) {
    const b = document.createElement("button");
    b.textContent = p.name;
    b.classList.toggle("on", keyCfg.preset === id);
    b.onclick = () => { keyCfg.preset = id; keyCfg.custom = {}; keySave(); keysRender(); };
    $("keysPreset").append(b);
  }
  $("keysXf").value = keyCfg.xfadeMs;
  box.innerHTML = "";
  let group = null;
  for (const [id, label, g] of ACTIONS) {
    if (g !== group) {
      group = g;
      const h = document.createElement("div");
      h.className = "kgroup";
      h.textContent = g;
      box.append(h);
    }
    const row = document.createElement("div");
    row.className = "krow" + (keyListen === id ? " listen" : "") + (keyCfg.custom[id] ? " custom" : "");
    const keys = keysFor(id);
    row.innerHTML = `<span>${esc(label)}</span><button class="kchip" title="Click, then press the new key (Esc: cancel, Backspace: none)">${
      keyListen === id ? "Press a key…" : keys.length ? keys.map((k) => `<kbd>${esc(comboLabel(k))}</kbd>`).join(" ") : "<i>none</i>"}</button>`;
    row.querySelector(".kchip").onclick = () => { keyListen = keyListen === id ? null : id; keysRender(); };
    box.append(row);
  }
}

function keySave() { store.set("keys", keyCfg); }

function keysSetup() {
  const saved = store.get("keys", {});
  if (saved && typeof saved === "object") Object.assign(keyCfg, saved);
  $("keysX").onclick = () => { keyListen = null; $("keysM").hidden = true; };
  $("keysM").onclick = (e) => { if (e.target === $("keysM")) { keyListen = null; $("keysM").hidden = true; } };
  $("keysReset").onclick = () => { keyCfg.custom = {}; keySave(); keysRender(); };
  $("keysXf").onchange = () => { keyCfg.xfadeMs = Math.max(5, Math.min(2000, +$("keysXf").value || 50)); keySave(); };
  // the key for the action that's listening (in the capture phase: before anything else gets it)
  addEventListener("keydown", (e) => {
    if ($("keysM").hidden || !keyListen) {
      if (!$("keysM").hidden && e.key === "Escape") { $("keysM").hidden = true; e.preventDefault(); }
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    const id = keyListen;
    if (e.key === "Escape") { keyListen = null; keysRender(); return; }
    if (e.key === "Backspace" && !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey && id !== "delete") {
      keyCfg.custom[id] = []; keyListen = null; keySave(); keysRender(); return;
    }
    const combo = comboOf(e);
    if (!combo) return;
    // a key does one thing: whatever had it lets go
    for (const a of ACTIONS) {
      if (a[0] !== id && keysFor(a[0]).includes(combo)) {
        keyCfg.custom[a[0]] = keysFor(a[0]).filter((k) => k !== combo);
        toast(`${comboLabel(combo)} was "${a[1]}": that one has ${keyCfg.custom[a[0]].length ? "its other key now" : "no key now"}.`);
      }
    }
    keyCfg.custom[id] = [combo];
    keyListen = null;
    keySave();
    keysRender();
  }, true);
}
