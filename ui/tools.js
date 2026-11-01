// Duet — the playlist's tools (like Logic's): pointer, pencil, scissors, glue, eraser, mute, fade.
// The cursor shows the tool. Keys 1–7 pick one; the pointer is the default. Also here: whether
// pattern blocks loop when you drag them longer, or make the pattern itself longer.

const cur = (svg, x, y, fallback) =>
  `url("data:image/svg+xml;utf8,${encodeURIComponent(`<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24' fill='none' stroke-linecap='round' stroke-linejoin='round'><g stroke='%23000' stroke-width='4.5'>${svg}</g><g stroke='%23fff' stroke-width='2'>${svg}</g></svg>`).replace(/%2523/g, "%23")}") ${x} ${y}, ${fallback}`;
const ICON = {
  pointer: "<path d='M5 3l14 8-6 1.5L10 19z'/>",
  pencil: "<path d='M4 20l1-5L16 4l4 4L9 19z'/><path d='M14 6l4 4'/>",
  scissors: "<circle cx='6' cy='17' r='3'/><circle cx='18' cy='17' r='3'/><path d='M8 15L19 3M16 15L5 3'/>",
  glue: "<path d='M9 3h6v4H9z'/><path d='M8 7h8l1 4v9H7v-9z'/>",
  eraser: "<path d='M3 16l9-9 7 7-6 6H7z'/><path d='M9 20h11'/>",
  mute: "<path d='M4 9h4l5-4v14l-5-4H4z'/><path d='M17 9l4 6M21 9l-4 6'/>",
  fade: "<path d='M3 20C9 20 15 12 21 4'/><path d='M3 20h18'/>",
};
const TOOLS = [
  ["pointer", "Pointer: select, move, trim, fades at the top corners", "1", 5, 3, "default"],
  ["pencil", "Pencil: click empty space to put the open pattern there (or paste)", "2", 4, 20, "crosshair"],
  ["scissors", "Scissors: click a clip to cut it there (all the selected ones under the mouse)", "3", 12, 10, "crosshair"],
  ["glue", "Glue: click a clip to join it to the one right after it (pieces of one recording or one pattern)", "4", 12, 4, "pointer"],
  ["eraser", "Eraser: click or drag over clips to delete them", "5", 4, 16, "not-allowed"],
  ["mute", "Mute: click a clip to mute or unmute it", "6", 12, 12, "pointer"],
  ["fade", "Fade: drag from a clip's start (fade in) or its end (fade out)", "7", 3, 20, "col-resize"],
];
const toolCursor = (t = song.tool) => { const d = TOOLS.find((x) => x[0] === t); return cur(ICON[t], d[3], d[4], d[5]); };

function setTool(t) {
  song.tool = TOOLS.some((x) => x[0] === t) ? t : "pointer";
  store.set("plTool", song.tool);
  for (const b of document.querySelectorAll("#plTools .tool")) b.classList.toggle("on", b.dataset.t === song.tool);
  $("lanes").style.cursor = song.tool === "pointer" ? "" : toolCursor();
}

function setPatLoop(on) {
  song.patLoop = !!on;
  store.set("patLoop", song.patLoop);
  $("patLoopBtn")?.classList.toggle("on", song.patLoop);
}

function toolsSetup() {
  const box = $("plTools");
  if (!box) return;
  for (const [t, tip, key] of TOOLS) {
    const b = document.createElement("button");
    b.className = "tool";
    b.dataset.t = t;
    b.title = `${tip} (${key})`;
    b.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICON[t]}</svg>`;
    b.onclick = () => setTool(t);
    box.append(b);
  }
  const lp = document.createElement("button");
  lp.className = "tool loopt";
  lp.id = "patLoopBtn";
  lp.title = "Pattern blocks loop when you drag them longer. Off: dragging a block's end makes the pattern itself longer (more bars to write in the piano roll)";
  lp.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 2l3 3-3 3"/><path d="M3 11V9a4 4 0 0 1 4-4h13"/><path d="M7 22l-3-3 3-3"/><path d="M21 13v2a4 4 0 0 1-4 4H4"/></svg><span>Loop</span>`;
  lp.onclick = () => { setPatLoop(!song.patLoop); toast(song.patLoop ? "Pattern blocks loop when you drag them longer." : "Dragging a pattern block longer now makes the pattern longer."); };
  box.append(lp);
  setTool(store.get("plTool", "pointer"));
  setPatLoop(store.get("patLoop", true));
  addEventListener("keydown", (e) => {
    if (ui.view !== "song" || e.ctrlKey || e.metaKey || e.altKey || e.target.closest("input, select, textarea")) return;
    if (!$("clipEd").hidden || !$("keysM").hidden || !$("newTrackM").hidden) return;
    const t = TOOLS.find((x) => x[2] === e.key);
    if (t) { e.preventDefault(); setTool(t[0]); }
  });
}

// the mouse went down on the lanes with a tool other than the pointer: true if the tool took it
function toolDown(e, p) {
  const t = song.tool, h = hitZone(p);
  if (t === "pencil") {
    if (h) return false; // on a clip the pencil moves and trims like the pointer
    const row = song.view.tracks[p.row];
    if (row && row.inst && curPattern()) { insertPattern(p); return true; }
    if (song.clipboard) { paste(Math.max(0, snapBeat(p.beat, snapFor(e))), p.row); return true; }
    return false;
  }
  if (t === "scissors") {
    if (!h) return true;
    const at = snapBeat(p.beat, snapFor(e));
    const under = song.sel.has(h.c.id) ? selClips().filter((k) => at > k.start && at < k.start + clipBeats(k)) : [h.c];
    if (!splitClips(at, under)) toast("Too close to the clip's edge to cut there.");
    return true;
  }
  if (t === "glue") {
    if (h) glueClip(h.c);
    return true;
  }
  if (t === "mute") {
    if (h) songEdit([["clip", h.c.id, { mute: !h.c.mute }]]);
    return true;
  }
  if (t === "eraser") {
    const lanes = $("lanes"), gone = new Set();
    const hit = (ev) => {
      const q = lanePoint(ev), z = q.inside && q.lane < 0 ? hitZone(q) : null;
      if (z && !gone.has(z.c.id)) { gone.add(z.c.id); z.c._erasing = true; songDraw(); }
    };
    const up = () => {
      lanes.removeEventListener("pointermove", hit); lanes.removeEventListener("pointerup", up);
      for (const c of song.view.clips) delete c._erasing;
      if (gone.size) { song.sel = gone; deleteSel(); }
    };
    lanes.addEventListener("pointermove", hit); lanes.addEventListener("pointerup", up);
    hit(e);
    return true;
  }
  if (t === "fade") {
    if (!h || h.c.pattern) return true;
    const c = h.c, lanes = $("lanes"), s0 = b2s(c.start), s1 = s0 + c.len;
    const which = b2s(p.beat) - s0 < c.len / 2 ? "fade_in" : "fade_out";
    const before = { fade_in: c.fade_in || 0, fade_out: c.fade_out || 0 };
    const move = (ev) => {
      const s = b2s(lanePoint(ev).beat);
      if (which === "fade_in") c.fade_in = Math.max(0, Math.min(c.len - (c.fade_out || 0), s - s0));
      else c.fade_out = Math.max(0, Math.min(c.len - (c.fade_in || 0), s1 - s));
      songDraw();
    };
    const up = () => {
      lanes.removeEventListener("pointermove", move); lanes.removeEventListener("pointerup", up);
      const v = c[which];
      Object.assign(c, before);
      songEdit([["clip", c.id, { [which]: v }]]);
    };
    lanes.addEventListener("pointermove", move); lanes.addEventListener("pointerup", up);
    move(e);
    return true;
  }
  return false;
}

// glue: a clip and the one that starts where it ends, on the same track, when they're one piece of
// sound (the same recording, carrying on) or one pattern carrying on
function glueClip(c) {
  const end = c.start + clipBeats(c), eps = 0.02;
  const next = song.view.clips.filter((k) => k.track === c.track && k.id !== c.id && Math.abs(k.start - end) < eps)
    .sort((a, b) => a.start - b.start)[0];
  if (!next) { toast("Nothing starts right where this clip ends to glue it to."); return; }
  if (c.pattern || next.pattern) {
    if (c.pattern !== next.pattern || Math.abs((next.boff || 0) - ((c.boff || 0) + c.blen)) > 1e-6) {
      toast("Only a pattern that carries on can be glued (the next block starts where this one stops in the pattern)."); return;
    }
    songEdit([["clip", c.id, { blen: c.blen + next.blen }], ["clip", next.id, { del: true }]]);
    return;
  }
  const same = ["file", "speed", "pitch", "stretch", "reverse"].every((f) => (c[f] ?? null) === (next[f] ?? null) || (f === "speed" && (c.speed ?? 1) === (next.speed ?? 1)) || (f === "pitch" && (c.pitch || 0) === (next.pitch || 0)) || ((f === "stretch" || f === "reverse") && !!c[f] === !!next[f]));
  const r = clipRatio(c), carry = c.reverse ? (next.offset || 0) + next.len * r : (c.offset || 0) + c.len * r;
  const want = c.reverse ? (c.offset || 0) : (next.offset || 0);
  if (!same || Math.abs(carry - want) > 0.005) {
    toast("Only pieces of the same recording that carry on can be glued (two different sounds would need a bounce)."); return;
  }
  songEdit([["clip", c.id, { len: c.len + next.len, fade_out: next.fade_out || 0, ...(c.reverse ? { offset: next.offset || 0 } : {}) }],
    ["clip", next.id, { del: true }]]);
}

// notes past a pattern's end: the pattern grows to hold them (to the bar); with looping off its blocks
// that showed the whole pattern grow with it
function growPattern(p, end) {
  if (!p || end <= patLen(p) + 1e-6) return [];
  const old = patLen(p), len = Math.ceil(end / bpb() - 1e-9) * bpb();
  const ch = [["pattern", p.id, { len }]];
  if (!song.patLoop) {
    for (const c of song.view.clips) if (c.pattern === p.id && !(c.boff > 0) && Math.abs((c.blen ?? old) - old) < 1e-6) ch.push(["clip", c.id, { blen: len }]);
  }
  return ch;
}
