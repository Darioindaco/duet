"use strict";
// Duet: working with your partner.
//  - Together / Apart (the button in the partner pill): together, play / stop and what you play live
//    are shared; apart, each of you plays and hears your own (the song itself, your pointers and
//    talkback stay shared). It switches for both of you.
//  - Edit locks: the pattern open in your piano roll (or channel rack), the clip in your clip editor or
//    Tune window is yours while you work on it: your partner can watch but not change it, and the other
//    way round. Idle for a minute and a half: free again.
//  - Coming back together after you both changed the song apart: a choice, not a silent mix.

const collab = { lastInput: performance.now() };

function collabSetup() {
  for (const ev of ["pointerdown", "keydown", "wheel"]) document.addEventListener(ev, () => (collab.lastInput = performance.now()), { capture: true, passive: true });
  $("pMode").onclick = async () => {
    const next = song.view?.mode === "apart" ? "together" : "apart";
    setSongView(await api.daw_mode(next));
    toast(next === "apart" ? "Apart: your own play and sound now (for your partner too). The song is still shared."
      : "Together: play, stop and live sound are shared again.");
  };
  for (const b of document.querySelectorAll("#rcModal [data-c]")) b.onclick = async () => {
    const c = b.dataset.c;
    if (c === "theirs" && !confirm("Use your partner's version for both of you? Yours is kept as a version (Song menu → Versions).")) return;
    $("rcModal").hidden = true;
    const r = await api.daw_reconcile(c);
    if (r && r.error) toast(r.error, true); else if (r) setSongView(r);
  };
}

// what you're editing now (for the lock), unless you've been idle for a while
function collabEdit() {
  if (performance.now() - collab.lastInput > 90000) return null;
  if (!$("clipEd").hidden && ce.id) return { kind: "clip", id: ce.id };
  if (typeof tw !== "undefined" && tw.clipId && tw.el && !tw.el.hidden) return { kind: "clip", id: tw.clipId };
  if (seq.pat && (winShown("roll") || winShown("rack"))) return { kind: "pattern", id: seq.pat };
  return null;
}

const patternLocked = (pid) => !!(pid && song.view?.locks?.pattern?.includes(pid));
const clipLocked = (cid) => !!(cid && song.view?.locks?.clip?.includes(cid));

// with every new song view
function collabPaint() {
  const v = song.view;
  if (!v) return;
  const apart = v.mode === "apart";
  const b = $("pMode");
  b.classList.toggle("apart", apart);
  b.title = apart ? "Apart: each of you plays and hears your own. Click: Together (shared play and sound)"
    : "Together: play, stop and what you play live are shared. Click: Apart (each your own play and sound; the song stays shared)";
  b.setAttribute("aria-label", apart ? "Apart" : "Together");
  $("pHere").classList.toggle("apart", apart);
  // the lock notes
  const lockNote = (id, on, what) => {
    let el = $(id);
    if (!el) return;
    el.hidden = !on;
    if (on) el.textContent = `🔒 Your partner is editing this ${what}: watch along, it's free once they're done`;
  };
  lockNote("prLock", patternLocked(seq.pat), "pattern");
  lockNote("ceLock", !$("clipEd").hidden && clipLocked(ce.id), "clip");
  lockNote("twLock", typeof tw !== "undefined" && clipLocked(tw.clipId), "clip");
  // both changed it apart
  const rc = v.reconcile;
  $("rcModal").hidden = !rc;
  if (rc) {
    const when = rc.since ? new Date(rc.since * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) : "last time";
    const n = (k) => `${k} change${k === 1 ? "" : "s"}`;
    $("rcText").innerHTML = `Since you last had <b>${esc(rc.name || v.meta.name)}</b> the same (${esc(when)}), you made <b>${n(rc.mine)}</b> and your partner made <b>${n(rc.theirs)}</b>, each on your own. What now?`;
  }
}
