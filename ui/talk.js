"use strict";
// Duet: talkback. Talk to your partner on a mic of its own (an input of your interface), heard
// only by them: never in a track, a recording, the song or its export. While they talk, your music
// goes down 12 dB under their voice (the engine does that, see dsp.rs).
//   Hold the Talk button, or hold the ` key (left of 1): talk while held (push-to-talk).
//   A quick click on Talk: on until you click again.
//   Right-click Talk: choose the talk mic.

const talk = { held: false, latched: false, input: null, keyDown: false };

function talkSetup() {
  const b = $("talkBtn");
  let downAt = 0;
  b.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    if (talk.input == null) { e.stopPropagation(); talkChoose(e.clientX, e.clientY); return; } // (or the click would close it)
    downAt = performance.now();
    talk.held = true;
    talkSend();
    b.setPointerCapture(e.pointerId);
  });
  const up = () => {
    if (!talk.held) return;
    talk.held = false;
    // a quick click: on until the next one (or off again, if it was on)
    if (performance.now() - downAt < 250) talk.latched = !talk.latched;
    talkSend();
  };
  b.addEventListener("pointerup", up);
  b.addEventListener("pointercancel", up);
  b.addEventListener("contextmenu", (e) => { e.preventDefault(); talkChoose(e.clientX, e.clientY); });
  // the ` key: push-to-talk (not while typing)
  document.addEventListener("keydown", (e) => {
    if (e.code !== "Backquote" || e.repeat || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.target.closest("input, textarea, select, [contenteditable]")) return;
    e.preventDefault();
    if (talk.input == null) { toast("Choose your talk mic first: right-click Talk."); return; }
    talk.keyDown = true;
    talkSend();
  });
  document.addEventListener("keyup", (e) => {
    if (e.code !== "Backquote" || !talk.keyDown) return;
    talk.keyDown = false;
    talkSend();
  });
  // let go when the window loses focus (a key held while switching apps never comes up)
  window.addEventListener("blur", () => { if (talk.keyDown || talk.held) { talk.keyDown = talk.held = false; talkSend(); } });
}

const talking = () => talk.held || talk.latched || talk.keyDown;

function talkSend() {
  api.talk?.(talking());
  talkPaint(null);
}

// the menu of inputs to talk on
function talkChoose(x, y) {
  const n = typeof inputCount === "function" ? inputCount() : 2;
  const items = [[(talk.input == null ? "✓ " : "   ") + "No talk mic", "", () => talkSet(null)]];
  for (let i = 0; i < n; i++) items.push([(talk.input === i ? "✓ " : "   ") + `Talk on input ${i + 1}`, "", () => talkSet(i)]);
  showCtx(items, x, y);
}

async function talkSet(i) {
  const r = await api.talk_config?.(i);
  talk.input = r ? r.input : i;
  if (talk.input == null) { talk.latched = false; talkSend(); }
  toast(talk.input == null ? "Talkback off" : `Talk mic: input ${talk.input + 1}. Hold Talk (or the \` key) to talk.`);
  talkPaint(null);
}

// every state poll: whether I'm talking, whether they are
function talkPaint(st) {
  if (st) {
    talk.input = st.input ?? null;
    talk.rx = st.rx || 0;
  }
  const b = $("talkBtn");
  if (!b) return;
  const me = talking(), them = (talk.rx || 0) > 0.02;
  b.classList.toggle("on", me);
  b.classList.toggle("them", them && !me);
  $("talkTxt").textContent = me ? (talk.latched ? "Talking (on)" : "Talking") : them ? "Partner talks" : "Talk";
  b.setAttribute("aria-label", $("talkTxt").textContent);
}
