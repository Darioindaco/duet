"use strict";
// Duet: seeing each other work. Where you are (the view, the pattern and
// channel you have open, what you've selected, your pointer, how far you've
// scrolled) goes to your partner a few times a second; theirs comes back with
// every state poll and shows in their colour: their pointer and selection in
// the playlist and the piano roll, the pattern and channel they're on, the
// channel they're playing live. Follow keeps your view on theirs until you do
// something yourself.
//
// Loaded before the main script; runs after boot() calls hereSetup().

const here = {
  ptr: null, sent: "", sentAt: 0,           // yours
  them: null, notes: {}, sig: "",            // theirs: presence, notes they hold per track
  follow: false, applying: false,
  disp: null,                                // their pointer as drawn (it glides to where it is)
};

function hereSetup() {
  const lanes = $("lanes"), pr = $("prLanes");
  lanes.addEventListener("pointermove", (e) => {
    const p = lanePoint(e), t = song.view && song.view.tracks[p.row];
    here.ptr = t ? { in: "song", beat: p.beat, track: t.id } : null;
  });
  pr.addEventListener("pointermove", (e) => { const p = prPoint(e); here.ptr = { in: "roll", beat: p.beat, key: p.key }; });
  for (const el of [lanes, pr]) el.addEventListener("pointerleave", () => (here.ptr = null));
  // anything you do yourself ends following
  const own = () => { if (here.follow && !here.applying) setFollow(false, "You're on your own again: stopped following."); };
  for (const id of ["lanes", "prLanes", "ruler", "viewSeg", "chanRows", "patSel", "theadsIn"]) $(id).addEventListener("pointerdown", own, true);
  for (const el of [lanes, pr]) el.addEventListener("wheel", own, { passive: true });
  $("pFollow").onclick = () => setFollow(!here.follow);
  $("pHereTxt").onclick = () => here.them && goTo(here.them);
  $("seqWho").onclick = () => here.them && goTo(here.them);
  setInterval(hereSend, 100);
}

// ---------- yours, to the partner ----------
const r2 = (x) => Math.round(x * 100) / 100;
function hereState() {
  const lanes = $("lanes"), pr = $("prLanes");
  const p = here.ptr && { ...here.ptr, beat: r2(here.ptr.beat) };
  return {
    view: ui.view, pattern: seq.pat, track: ui.view === "seq" ? seq.channel : song.selTrack,
    sel: [...song.sel].slice(0, 64), notes: [...seq.sel].slice(0, 64), ptr: p,
    scroll: { beat: r2(lanes.scrollLeft / song.ppb), row: r2(lanes.scrollTop / ROW) },
    roll: { beat: r2(pr.scrollLeft / seq.ppb), key: prKeyAt(pr.scrollTop + pr.clientHeight / 2) },
  };
}

function hereSend() {
  if (!song.view || !song.view.shared) return;
  const s = JSON.stringify(hereState()), now = performance.now();
  if (s === here.sent && now - here.sentAt < 2000) return; // and every 2 s, so it doesn't go stale
  here.sent = s; here.sentAt = now;
  api.daw_presence(JSON.parse(s));
}

// ---------- theirs, from the poll ----------
function herePaint(them, notes) {
  const sig = JSON.stringify([them, notes]);
  if (sig === here.sig) return;
  here.sig = sig; here.them = them; here.notes = notes || {};
  $("pHere").hidden = !them;
  if (!them) {
    if (here.follow) setFollow(false, "Your partner is gone for now: stopped following.");
    here.disp = null;
  } else {
    const label = hereLabel(them);
    $("pHereTxt").textContent = label;
    $("pHereTxt").title = label + " · click to go there";
    $("pHere").title = label;
  }
  hereMarks();
  seqWho();
  if (here.follow && them) goTo(them);
  if (song.view && winShown("song")) { drawOverlay(); drawRuler(); }
  if (song.view && winShown("roll")) prDraw();
}

const trackName = (id) => song.view?.tracks.find((t) => t.id === id)?.name;

function hereLabel(t) {
  const playing = Object.keys(here.notes).map(trackName).filter(Boolean);
  if (playing.length) return `Partner · playing ${playing.join(", ")}`;
  if (t.view === "seq") {
    const p = patterns().find((x) => x.id === t.pattern);
    return ["Partner · Sequencer", p && (p.name || "Pattern"), trackName(t.track)].filter(Boolean).join(" · ");
  }
  if (t.view === "mixer") return "Partner · Mixer";
  const n = t.sel.length;
  return `Partner · Playlist${n ? ` · ${n} clip${n === 1 ? "" : "s"} selected` : ""}`;
}

// classes on things the page rebuilds: put them back after every rebuild too
function hereMarks() {
  const t = here.them;
  for (const el of document.querySelectorAll(".th, .chan")) el.classList.toggle("plive", !!here.notes[el.dataset.id]);
  for (const el of document.querySelectorAll(".chan")) el.classList.toggle("phere", !!t && t.view === "seq" && t.track === el.dataset.id);
  const held = here.notes[seq.channel] || [];
  for (const el of document.querySelectorAll(".pkey")) el.classList.toggle("plive", held.includes(+el.dataset.k));
}

// in the sequencer: which pattern they're on (a click takes you there)
function seqWho() {
  const t = here.them, el = $("seqWho");
  const p = t && t.view === "seq" && patterns().find((x) => x.id === t.pattern);
  el.hidden = !p;
  if (!p) return;
  const same = p.id === seq.pat;
  el.classList.toggle("same", same);
  el.textContent = same ? "Partner is in this pattern" : `Partner is in ${p.name || "a pattern"} ›`;
  el.title = same ? "" : "Go to the pattern your partner has open";
}

// ---------- going where they are ----------
function setFollow(on, why) {
  here.follow = on && !!here.them;
  $("pFollow").classList.toggle("on", here.follow);
  $("pFollow").textContent = here.follow ? "Following" : "Follow";
  if (here.follow) goTo(here.them);
  else if (why) toast(why);
}

function goTo(t) {
  here.applying = true;
  try {
    if (ui.view !== t.view) setMainView(t.view);
    if (t.view === "seq") {
      let changed = false;
      if (t.pattern && t.pattern !== seq.pat && patterns().some((p) => p.id === t.pattern)) { seq.pat = t.pattern; changed = true; }
      if (t.track && t.track !== seq.channel && channels().some((c) => c.id === t.track)) { seq.channel = t.track; seq.sel.clear(); changed = true; }
      if (changed) { seq.prTop = 0; seqRender(); syncMidiTarget(); }
      if (t.roll) requestAnimationFrame(() => {
        const pr = $("prLanes"), ch = prChannel();
        if (!ch) return;
        const [, hi] = prRange(ch);
        pr.scrollLeft = t.roll.beat * seq.ppb;
        pr.scrollTop = seq.prTop = Math.max(0, (hi - t.roll.key + 0.5) * seq.rowH - pr.clientHeight / 2);
      });
    } else if (t.view === "song" && t.scroll) {
      requestAnimationFrame(() => {
        const lanes = $("lanes");
        const x = t.scroll.beat * song.ppb, y = t.scroll.row * ROW;
        if (Math.abs(lanes.scrollLeft - x) < 1 && Math.abs(lanes.scrollTop - y) < 1) return;
        song.autoScroll = true; // not you scrolling: the playhead keeps being followed
        lanes.scrollLeft = x; lanes.scrollTop = y;
      });
    }
  } finally {
    here.applying = false;
  }
}

// ---------- drawing ----------
// their pointer glides to where it is (updates come ~10 times a second)
function hereFrame() {
  // shown wherever that window is open (with windows, more than one can be)
  const t = here.them, p = t && t.ptr && winShown(t.ptr.in === "song" ? "song" : "roll") ? t.ptr : null;
  if (!p) { if (here.disp) { here.disp = null; redrawThem(); } return; }
  const y = p.in === "song" ? trackRow(p.track) : p.key;
  const d = here.disp;
  if (!d || d.in !== p.in || Math.abs(d.y - y) > 8) { here.disp = { in: p.in, beat: p.beat, y }; redrawThem(); return; }
  const db = p.beat - d.beat, dy = y - d.y;
  if (Math.abs(db) < 1e-3 && Math.abs(dy) < 1e-3) return;
  d.beat += db * 0.35; d.y += dy * 0.35;
  if (Math.abs(db) < 0.01 && Math.abs(dy) < 0.01) { d.beat = p.beat; d.y = y; }
  redrawThem();
}
function redrawThem() {
  if (winShown("song") && !(song.st && song.st.playing)) drawOverlay(); // while playing it's drawn every frame anyway
  if (winShown("roll")) prDraw();
}

// on the playlist overlay: their selection and their pointer
function hereDrawSong(ctx, sx, sy, w, h) {
  const t = here.them;
  if (!t) return;
  const v = song.view, ppb = song.ppb;
  if (t.sel.length) {
    ctx.save();
    ctx.setLineDash([5, 4]);
    ctx.strokeStyle = song.colors.partner;
    ctx.lineWidth = 1.5;
    for (const c of v.clips) {
      if (!t.sel.includes(c.id)) continue;
      const row = trackRow(c.track);
      const x = c.start * ppb - sx, cw = clipBeats(c) * ppb, y = row * ROW - sy + 2;
      if (row < 0 || x > w || x + cw < 0 || y > h || y + ROW < 0) continue;
      ctx.beginPath(); ctx.roundRect(x - 1.5, y - 1.5, cw + 2, ROW - 3, 7); ctx.stroke();
    }
    ctx.restore();
  }
  const d = here.disp;
  if (d && d.in === "song" && d.y >= 0) hereCursor(ctx, d.beat * ppb - sx, d.y * ROW - sy + ROW / 2, w, h);
}

// in the piano roll: their selected notes and pointer, when they're on the same notes
function hereDrawRoll(ctx, sx, sy, hi, w, h) {
  const t = here.them;
  if (!t || t.view !== "seq" || t.pattern !== seq.pat || t.track !== seq.channel) return;
  const ppb = seq.ppb, rh = seq.rowH;
  if (t.notes.length) {
    ctx.save();
    ctx.setLineDash([4, 3]);
    ctx.strokeStyle = song.colors.partner;
    ctx.lineWidth = 1.5;
    for (const n of patNotes(seq.pat, seq.channel)) {
      if (!t.notes.includes(n.id)) continue;
      ctx.beginPath(); ctx.roundRect(n.start * ppb - sx - 1, (hi - n.key) * rh - sy + 0.5, Math.max(3, n.len * ppb - 1) + 3, rh - 1, 4); ctx.stroke();
    }
    ctx.restore();
  }
  const d = here.disp;
  if (d && d.in === "roll") hereCursor(ctx, d.beat * ppb - sx, (hi - d.y + 0.5) * rh - sy, w, h);
}

// the notes they're holding on this channel: their rows light up
function hereDrawHeld(ctx, sx, sy, w, hi) {
  const held = here.notes[seq.channel];
  if (!held) return;
  ctx.fillStyle = song.colors.partner + "33";
  for (const k of held) ctx.fillRect(0, (hi - k) * seq.rowH - sy, w, seq.rowH);
}

// their pointer with a name tag; out of view, a tag on the edge pointing to it
function hereCursor(ctx, x, y, w, h) {
  const col = song.colors.partner;
  ctx.save();
  ctx.font = "700 10.5px system-ui, sans-serif";
  if (x < 0 || y < 0 || x > w - 4 || y > h - 4) {
    const label = "Partner " + (y < 0 ? "↑" : y > h - 4 ? "↓" : x < 0 ? "←" : "→");
    const tw = ctx.measureText(label).width + 14;
    const cx = Math.max(6, Math.min(w - tw - 6, x - tw / 2)), cy = Math.max(6, Math.min(h - 23, y - 8));
    ctx.fillStyle = col;
    ctx.beginPath(); ctx.roundRect(cx, cy, tw, 17, 8.5); ctx.fill();
    ctx.fillStyle = "#04201d";
    ctx.fillText(label, cx + 7, cy + 12);
    ctx.restore();
    return;
  }
  ctx.translate(Math.round(x), Math.round(y));
  ctx.fillStyle = col;
  ctx.strokeStyle = "#0b0a0f";
  ctx.lineWidth = 1.2;
  ctx.beginPath();
  ctx.moveTo(0, 0); ctx.lineTo(0, 15); ctx.lineTo(4, 11.5); ctx.lineTo(7, 18); ctx.lineTo(9.5, 17); ctx.lineTo(6.5, 10.5); ctx.lineTo(11.5, 10.5);
  ctx.closePath(); ctx.fill(); ctx.stroke();
  const label = "Partner", tw = ctx.measureText(label).width;
  ctx.beginPath(); ctx.roundRect(11, 15, tw + 12, 17, 8.5); ctx.fill();
  ctx.fillStyle = "#04201d";
  ctx.fillText(label, 17, 27);
  ctx.restore();
}

// on the ruler: where their playhead stands while stopped (when it's not where yours is)
function hereDrawRuler(ctx, sx) {
  const t = here.them, st = song.st;
  if (!t || (st && st.playing) || Math.abs(t.pos - curPos()) < 1e-3) return;
  const px = t.pos * song.ppb - sx;
  ctx.fillStyle = song.colors.partner;
  ctx.beginPath(); ctx.moveTo(px - 4, 14); ctx.lineTo(px + 4, 14); ctx.lineTo(px, 21); ctx.fill();
}
