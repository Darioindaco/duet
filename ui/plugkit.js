// Duet — the plugin kit: what every built-in effect's window is made of.
//
// A window has a bar (presets with ◀ ▶, save, A/B with copy, undo/redo, reset), a display (a
// graph of what the effect does, drawn live: the EQ's curve with handles to drag, a
// compressor's transfer curve with the signal moving on it, a limiter's gain reduction over
// time, the echoes of a delay, a reverb's tail, an LFO), meters (in, out, gain reduction) and
// knobs. The knobs: drag (Shift: fine), scroll, double-click for the default, Alt/Ctrl-click
// to type a value, right-click to automate. Values map to the engine's 0..1 knobs exactly as
// fx.rs does (PK_SPEC).

"use strict";

// ---------- what each knob means ----------
// [key, label, kind, a, b, unit, extra]: lin a..b, log a..b, pct; "off" values at the ends
const PK_SPEC = {
  eq: [["lowcut", "Low cut", "log", 20, 800, "Hz", { offBelow: 0.005 }], ["low", "Low", "lin", -15, 15, "dB"], ["lowfreq", "Low freq", "log", 30, 600, "Hz"],
    ["mid", "Mid", "lin", -15, 15, "dB"], ["midfreq", "Mid freq", "log", 100, 10000, "Hz"], ["q", "Q", "log", 0.3, 9.99, ""],
    ["high", "High", "lin", -15, 15, "dB"], ["highfreq", "High freq", "log", 1500, 15000, "Hz"], ["highcut", "High cut", "log", 1000, 20000, "Hz", { offAbove: 0.995 }]],
  comp: [["threshold", "Threshold", "lin", -40, -4, "dB"], ["ratio", "Ratio", "lin", 1, 12, ":1"], ["attack", "Attack", "log", 0.1, 100, "ms"],
    ["release", "Release", "log", 10, 1000, "ms"], ["makeup", "Makeup", "lin", 0, 18, "dB"]],
  limiter: [["gain", "Input gain", "lin", 0, 24, "dB"], ["ceiling", "Ceiling", "lin", -12, 0, "dB"], ["release", "Release", "log", 10, 1000, "ms"]],
  drive: [["drive", "Drive", "pct"], ["tone", "Tone", "pct"], ["level", "Level", "pct"]],
  chorus: [["rate", "Rate", "lin", 0.1, 3.1, "Hz"], ["depth", "Depth", "pct"], ["mix", "Mix", "pct"]],
  tremolo: [["rate", "Rate", "lin", 0.5, 12, "Hz"], ["depth", "Depth", "pct"], ["stereo", "Stereo", "pct"]],
  delay: [["time", "Time", "lin", 60, 1160, "ms"], ["feedback", "Feedback", "pct"], ["mix", "Mix", "pct"], ["pingpong", "Ping-pong", "pct"], ["dry", "Dry", "pct"]],
  reverb: [["size", "Size", "pct"], ["damp", "Damping", "pct"], ["width", "Width", "pct"], ["mix", "Mix", "pct"], ["dry", "Dry", "pct"]],
  nam: [["input", "Input", "lin", -12, 12, "dB"], ["output", "Output", "lin", -12, 12, "dB"]],
  vst: [["mix", "Mix", "pct"]],
};
// which knobs sit together, and under which heading
const PK_GROUPS = {
  eq: [["Cuts", ["lowcut", "highcut"]], ["Low", ["low", "lowfreq"]], ["Mid", ["mid", "midfreq", "q"]], ["High", ["high", "highfreq"]]],
  comp: [["Detector", ["threshold", "ratio"]], ["Timing", ["attack", "release"]], ["Output", ["makeup"]]],
  limiter: [["", ["gain", "ceiling", "release"]]],
  delay: [["Echo", ["time", "feedback", "pingpong"]], ["Mix", ["mix", "dry"]]],
  reverb: [["Room", ["size", "damp", "width"]], ["Mix", ["mix", "dry"]]],
};

function pkSpec(kind, key) { return (PK_SPEC[kind] || []).find((s) => s[0] === key); }
function pkReal(spec, v) {
  const [, , k, a, b] = spec;
  return k === "log" ? a * (b / a) ** v : k === "lin" ? a + (b - a) * v : v * 100;
}
function pkNorm(spec, real) {
  const [, , k, a, b] = spec;
  const v = k === "log" ? Math.log(Math.max(1e-9, real) / a) / Math.log(b / a) : k === "lin" ? (real - a) / (b - a) : real / 100;
  return Math.max(0, Math.min(1, v));
}
function pkFmt(spec, v) {
  const x = spec[6] || {};
  if ((x.offBelow != null && v < x.offBelow) || (x.offAbove != null && v >= x.offAbove)) return "off";
  const r = pkReal(spec, v), u = spec[5] ?? "%";
  if (u === "Hz" && r < 10) return r.toFixed(2) + " Hz";
  if (u === "Hz") return r >= 1000 ? (r / 1000).toFixed(r >= 10000 ? 1 : 2) + " kHz" : Math.round(r) + " Hz";
  if (u === "ms") return r >= 1000 ? (r / 1000).toFixed(2) + " s" : r >= 10 ? Math.round(r) + " ms" : r.toFixed(1) + " ms";
  if (u === "dB") return (r > 0.05 ? "+" : "") + r.toFixed(1) + " dB";
  if (u === ":1") return r.toFixed(1) + ":1";
  if (u === "") return r.toFixed(2);
  return Math.round(r) + "%";
}

// ---------- factory presets ----------
const PK_PRESETS = {
  eq: { "Vocal presence": { lowcut: 0.3, low: 0.45, mid: 0.43, midfreq: 0.35, q: 0.4, high: 0.62, highfreq: 0.55 },
    "Kick punch": { lowcut: 0.05, low: 0.65, lowfreq: 0.3, mid: 0.35, midfreq: 0.3, q: 0.45, high: 0.58, highfreq: 0.4 },
    "Telephone": { lowcut: 0.6, highcut: 0.25, mid: 0.7, midfreq: 0.55, q: 0.3 },
    "Air": { high: 0.68, highfreq: 0.85 },
    "Clean up mud": { lowcut: 0.18, mid: 0.38, midfreq: 0.25, q: 0.35 } },
  comp: { "Gentle glue": { threshold: 0.6, ratio: 0.1, attack: 0.65, release: 0.55, makeup: 0.1 },
    "Vocal": { threshold: 0.45, ratio: 0.25, attack: 0.5, release: 0.45, makeup: 0.25 },
    "Drum smash": { threshold: 0.25, ratio: 0.8, attack: 0.45, release: 0.3, makeup: 0.45 },
    "Bass even": { threshold: 0.4, ratio: 0.35, attack: 0.55, release: 0.55, makeup: 0.25 },
    "Sidechain pump": { threshold: 0.3, ratio: 0.9, attack: 0.1, release: 0.5, makeup: 0 } },
  limiter: { "Transparent": { gain: 0.1, ceiling: 0.917, release: 0.5 }, "Loud master": { gain: 0.35, ceiling: 0.917, release: 0.35 },
    "Safety": { gain: 0, ceiling: 0.992, release: 0.6 } },
  delay: { "Slapback": { time: 0.05, feedback: 0.1, mix: 0.3, pingpong: 0 }, "Quarter echo": { time: 0.4, feedback: 0.35, mix: 0.3, pingpong: 0 },
    "Wide ping-pong": { time: 0.3, feedback: 0.45, mix: 0.35, pingpong: 1 }, "Dub": { time: 0.55, feedback: 0.75, mix: 0.4, pingpong: 0.5 } },
  reverb: { "Small room": { size: 0.25, damp: 0.5, mix: 0.2, width: 0.8 }, "Plate": { size: 0.55, damp: 0.25, mix: 0.3, width: 1 },
    "Big hall": { size: 0.9, damp: 0.35, mix: 0.35, width: 1 }, "Dark ambience": { size: 0.75, damp: 0.85, mix: 0.4, width: 1 } },
  drive: { "Warm": { drive: 0.25, tone: 0.4, level: 0.55 }, "Crunch": { drive: 0.55, tone: 0.6, level: 0.45 }, "Fuzz": { drive: 0.95, tone: 0.7, level: 0.35 } },
  chorus: { "Subtle": { rate: 0.15, depth: 0.3, mix: 0.3 }, "Lush": { rate: 0.25, depth: 0.7, mix: 0.5 }, "Vibrato": { rate: 0.5, depth: 0.6, mix: 1 } },
  tremolo: { "Slow": { rate: 0.15, depth: 0.5 }, "Choppy": { rate: 0.6, depth: 1 }, "Auto-pan": { rate: 0.2, depth: 0.7, stereo: 1 } },
};
const pkUserPresets = (kind) => store.get("presets." + kind, {});

// ---------- a knob ----------
// opts: label, value (0..1), def, spec, color, size, onChange(v), onGesture() (before a change starts: for undo)
function pkKnob(o) {
  const size = o.size || 54, el = document.createElement("div");
  el.className = "pkknob";
  el.tabIndex = 0;
  el.style.setProperty("--c", o.color);
  const r = size / 2 - 5, cx = size / 2, A0 = 135, SW = 270;
  const pt = (deg, rad) => [cx + rad * Math.cos(deg * Math.PI / 180), cx + rad * Math.sin(deg * Math.PI / 180)];
  const arc = (from, to, rad) => {
    const [x1, y1] = pt(from, rad), [x2, y2] = pt(to, rad);
    return `M${x1.toFixed(2)} ${y1.toFixed(2)}A${rad} ${rad} 0 ${to - from > 180 ? 1 : 0} 1 ${x2.toFixed(2)} ${y2.toFixed(2)}`;
  };
  // a bipolar knob (gain around 0) fills from the middle
  const bip = o.spec && o.spec[2] === "lin" && o.spec[3] < 0 && o.spec[4] > 0;
  const ticks = Array.from({ length: 11 }, (_, i) => { const [x1, y1] = pt(A0 + SW * i / 10, r + 3), [x2, y2] = pt(A0 + SW * i / 10, r + 5); return `M${x1} ${y1}L${x2} ${y2}`; }).join("");
  el.innerHTML = `<svg width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">
      <path d="${ticks}" stroke="rgba(255,255,255,0.14)" stroke-width="1"/>
      <path d="${arc(A0, A0 + SW, r)}" stroke="rgba(0,0,0,0.55)" stroke-width="5" fill="none" stroke-linecap="round"/>
      <path class="val" stroke="var(--c)" stroke-width="5" fill="none" stroke-linecap="round"/>
      <circle cx="${cx}" cy="${cx}" r="${r - 7}" class="cap"/>
      <line class="ptr" x1="${cx}" y1="${cx}" stroke="#fff" stroke-width="2.4" stroke-linecap="round"/>
    </svg><div class="pkv"></div><div class="pkl">${esc(o.label)}</div>`;
  const val = el.querySelector(".val"), ptr = el.querySelector(".ptr"), kv = el.querySelector(".pkv");
  let v = o.value;
  const render = () => {
    const deg = A0 + SW * v, mid = A0 + SW / 2;
    val.setAttribute("d", bip ? (Math.abs(v - 0.5) < 0.004 ? "" : v > 0.5 ? arc(mid, deg, r) : arc(deg, mid, r)) : v > 0.003 ? arc(A0, deg, r) : "");
    const [x, y] = pt(deg, r - 10);
    ptr.setAttribute("x2", x.toFixed(2)); ptr.setAttribute("y2", y.toFixed(2));
    kv.textContent = o.spec ? pkFmt(o.spec, v) : Math.round(v * 100) + "%";
  };
  const set = (nv) => {
    nv = Math.max(0, Math.min(1, nv));
    if (Math.abs(nv - v) < 1e-6) return;
    v = nv; render(); o.onChange(v);
  };
  el.setValue = (nv) => { v = Math.max(0, Math.min(1, nv)); render(); };
  const svg = el.querySelector("svg");
  svg.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    if (e.altKey || e.ctrlKey || e.metaKey) { e.preventDefault(); typeIn(); return; }
    o.onGesture?.();
    svg.setPointerCapture(e.pointerId);
    el.classList.add("drag");
    let lastY = e.clientY;
    const move = (ev) => { set(v + (lastY - ev.clientY) * (ev.shiftKey ? 0.0012 : 0.006)); lastY = ev.clientY; };
    const up = () => { el.classList.remove("drag"); svg.removeEventListener("pointermove", move); svg.removeEventListener("pointerup", up); };
    svg.addEventListener("pointermove", move);
    svg.addEventListener("pointerup", up);
  });
  svg.addEventListener("dblclick", () => { o.onGesture?.(); set(o.def ?? 0.5); });
  el.addEventListener("wheel", (e) => { e.preventDefault(); o.onGesture?.(); set(v - Math.sign(e.deltaY) * (e.shiftKey ? 0.004 : 0.02)); }, { passive: false });
  el.addEventListener("keydown", (e) => {
    const d = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1 }[e.key];
    if (d) { e.preventDefault(); o.onGesture?.(); set(v + d * (e.shiftKey ? 0.004 : 0.02)); }
    if (e.key === "Enter") { e.preventDefault(); typeIn(); }
  });
  kv.addEventListener("click", typeIn);
  function typeIn() {
    if (!o.spec) return;
    const inp = document.createElement("input");
    inp.className = "pkin";
    inp.value = String(+pkReal(o.spec, v).toFixed(2));
    kv.replaceWith(inp);
    inp.focus(); inp.select();
    let over = false;
    const done = (ok) => {
      if (over) return;
      over = true;
      if (ok) {
        const n = parseFloat(inp.value.replace(",", ".").replace(/[^\d.\-+e]/g, ""));
        if (Number.isFinite(n)) {
          // "2k" means 2000
          const real = /k/i.test(inp.value) && o.spec[5] === "Hz" ? n * 1000 : n;
          o.onGesture?.(); set(pkNorm(o.spec, real));
        }
      }
      inp.replaceWith(kv); render();
    };
    inp.addEventListener("keydown", (e) => { if (e.key === "Enter") done(true); if (e.key === "Escape") done(false); e.stopPropagation(); });
    inp.addEventListener("blur", () => done(true));
  }
  render();
  return el;
}

// ---------- a meter ----------
function pkMeter(label, kind = "level") {
  const el = document.createElement("div");
  el.className = "pkmeter " + kind;
  el.innerHTML = `<div class="bar"><i></i><b></b></div><span>${label}</span>`;
  const i = el.querySelector("i"), b = el.querySelector("b");
  let hold = 0, holdAt = 0;
  // level: 0..1 peak (shown -60..+6 dB); gr: dB turned down (0..24, from the top)
  el.set = (x) => {
    let frac;
    if (kind === "gr") frac = Math.min(1, x / 24);
    else { const db = x > 1e-6 ? 20 * Math.log10(x) : -90; frac = Math.max(0, Math.min(1, (db + 60) / 66)); }
    i.style.height = frac * 100 + "%";
    const now = performance.now();
    if (frac >= hold || now - holdAt > 1200) { hold = frac; holdAt = now; }
    b.style.bottom = kind === "gr" ? "" : hold * 100 + "%";
    b.style.top = kind === "gr" ? hold * 100 + "%" : "";
    el.classList.toggle("clip", kind !== "gr" && x >= 1);
  };
  return el;
}

// ---------- the window ----------
// w: the fx window record ({ el, t, pos }); slot: its rack slot (live object from the mixer)
function pkPanel(w, slot) {
  const kind = slot.kind, meta = FXMETA[kind], color = meta.color, t = w.t, pos = w.pos;
  const spec = PK_SPEC[kind] || [];
  const root = document.createElement("div");
  root.className = "pk";
  root.style.setProperty("--c", color);
  w.pk ||= { undo: [], redo: [], ab: null, which: "A", preset: null };
  const st = w.pk;
  const params = () => Object.fromEntries(spec.map(([k]) => [k, slot[k] ?? FX_DEFAULTS[kind][k]]));
  const knobs = {};
  // a change from a knob, a handle or a preset: into the engine and the mixer file
  const setParam = (k, v, from) => {
    slot[k] = v;
    api.rack_set(t, pos, k, v);
    if (from !== "knob") knobs[k]?.setValue(v);
    display?.redraw?.();
    saveMixer();
  };
  const gesture = () => {
    const now = performance.now();
    if (st.lastGesture && now - st.lastGesture < 400) { st.lastGesture = now; return; } // one turn, one undo step
    st.lastGesture = now;
    st.undo.push(params()); st.redo = []; if (st.undo.length > 100) st.undo.shift();
    bar.refresh();
  };
  const applyAll = (vals, name) => {
    for (const [k, v] of Object.entries(vals)) if (pkSpec(kind, k)) setParam(k, v);
    if (name !== undefined) st.preset = name;
    bar.refresh();
  };

  // --- the bar: presets, A/B, undo, reset
  const bar = document.createElement("div");
  bar.className = "pkbar";
  const all = () => ({ ...(PK_PRESETS[kind] || {}), ...pkUserPresets(kind) });
  const names = () => Object.keys(all());
  bar.innerHTML = `<button class="pkb prev" title="Previous preset">◀</button><button class="pkpre" title="Presets"></button><button class="pkb next" title="Next preset">▶</button>
    <button class="pkb save" title="Save as a preset">Save</button><span class="sp"></span>
    <button class="pkb ab a" title="Setting A (click again to copy A to B)">A</button><button class="pkb ab b" title="Setting B">B</button>
    <button class="pkb undo" title="Undo (in this effect)">↶</button><button class="pkb redo" title="Redo">↷</button>
    <button class="pkb reset" title="Back to the defaults">Reset</button>`;
  const pre = bar.querySelector(".pkpre");
  const step = (d) => { const n = names(); if (!n.length) return; const i = (n.indexOf(st.preset) + d + n.length) % n.length; gesture(); applyAll(all()[n[i]], n[i]); };
  bar.querySelector(".prev").onclick = () => step(-1);
  bar.querySelector(".next").onclick = () => step(1);
  pre.onclick = (e) => {
    const r = pre.getBoundingClientRect(), user = pkUserPresets(kind);
    const items = Object.keys(PK_PRESETS[kind] || {}).map((n) => [n, "", () => { gesture(); applyAll(all()[n], n); }]);
    for (const n of Object.keys(user)) items.push(["★ " + n, "", () => { gesture(); applyAll(user[n], n); }]);
    if (Object.keys(user).length) items.push(["Delete a saved preset…", "", () => {
      const n = prompt("Which of your presets to delete?", st.preset && user[st.preset] ? st.preset : Object.keys(user)[0]);
      if (n && user[n]) { delete user[n]; store.set("presets." + kind, user); bar.refresh(); }
    }]);
    if (!items.length) items.push(["No presets yet", "", () => {}]);
    showCtx(items, r.left, r.bottom + 2);
  };
  bar.querySelector(".save").onclick = () => {
    const n = (prompt("A name for this setting:", st.preset || "My " + meta.name) || "").trim();
    if (!n) return;
    const user = pkUserPresets(kind);
    user[n] = params();
    store.set("presets." + kind, user);
    st.preset = n; bar.refresh();
    toast(`Saved "${n}" with your ${meta.name} presets.`);
  };
  const abBtn = (which) => bar.querySelector(".ab." + which.toLowerCase());
  for (const which of ["A", "B"]) abBtn(which).onclick = () => {
    st.ab ||= { A: params(), B: params() };
    if (st.which === which) { // again: copy this one to the other
      st.ab[which === "A" ? "B" : "A"] = params();
      toast(`${which} copied to ${which === "A" ? "B" : "A"}.`);
      return;
    }
    st.ab[st.which] = params();
    st.which = which;
    gesture(); applyAll(st.ab[which]);
  };
  bar.querySelector(".undo").onclick = () => { if (!st.undo.length) return; st.redo.push(params()); applyAll(st.undo.pop()); };
  bar.querySelector(".redo").onclick = () => { if (!st.redo.length) return; st.undo.push(params()); applyAll(st.redo.pop()); };
  bar.querySelector(".reset").onclick = () => { gesture(); applyAll(FX_DEFAULTS[kind], null); };
  bar.refresh = () => {
    pre.textContent = st.preset || "Presets";
    abBtn("A").classList.toggle("on", st.which === "A");
    abBtn("B").classList.toggle("on", st.which === "B");
    bar.querySelector(".undo").disabled = !st.undo.length;
    bar.querySelector(".redo").disabled = !st.redo.length;
  };
  bar.refresh();
  root.append(bar);

  // --- the display and the meters
  const mid = document.createElement("div");
  mid.className = "pkmid";
  const display = PK_DISPLAY[kind]?.(slot, color, setParam, gesture, t, pos);
  if (display) mid.append(display);
  const meters = document.createElement("div");
  meters.className = "pkmeters";
  const mIn = pkMeter("In"), mOut = pkMeter("Out"), mGr = kind === "comp" || kind === "limiter" ? pkMeter("GR", "gr") : null;
  meters.append(mIn, ...(mGr ? [mGr] : []), mOut);
  mid.append(meters);
  root.append(mid);
  w.meter = (m) => { mIn.set(m[0]); mOut.set(m[1]); mGr?.set(m[2]); display?.live?.(m); };

  // --- the knobs, grouped
  const groups = PK_GROUPS[kind] || [["", spec.map((s) => s[0])]];
  const row = document.createElement("div");
  row.className = "pkknobs";
  for (const [title, keys] of groups) {
    const g = document.createElement("div");
    g.className = "pkgroup";
    if (title) g.innerHTML = `<div class="pkgt">${esc(title)}</div>`;
    const ks = document.createElement("div");
    ks.className = "pkgk";
    for (const k of keys) {
      const s = pkSpec(kind, k);
      if (!s) continue;
      const kn = pkKnob({ label: s[1], value: slot[k] ?? FX_DEFAULTS[kind][k], def: FX_DEFAULTS[kind][k], spec: s, color,
        onGesture: gesture, onChange: (v) => setParam(k, v, "knob") });
      knobs[k] = kn;
      ks.append(autoOnControl(kn, t, `fx:${pos}:${kind}:${k}`));
    }
    g.append(ks);
    row.append(g);
  }
  root.append(row);
  // what only some effects have: the compressor's sidechain, errors
  if (kind === "comp" && (t[0] === "T" || t === "M")) root.append(keySelect(t, pos, slot));
  if (slot.error) root.insertAdjacentHTML("afterbegin", `<div class="mnote bad">${esc(slot.error)}</div>`);
  root.classList.toggle("off", !slot.on);
  return root;
}

// the meters of every open window, a dozen times a second
setInterval(async () => {
  if (typeof fxw === "undefined" || !fxw.size || !api.rack_meters) return;
  const strips = new Set([...fxw.values()].filter((w) => w.meter).map((w) => w.t));
  for (const t of strips) {
    let ms;
    try { ms = await api.rack_meters(t); } catch (e) { continue; }
    for (const w of fxw.values()) if (w.t === t && w.meter && ms[w.pos]) w.meter(ms[w.pos]);
  }
}, 80);

// ---------- the displays ----------
function pkCanvas(cls, draw, h = 150) {
  const cv = document.createElement("canvas");
  cv.className = "pkdisp " + cls;
  cv.style.height = h + "px";
  cv.redraw = () => {
    const w = cv.clientWidth || 360, hh = cv.clientHeight || h;
    const ctx = sizeCanvas(cv, w, hh, false);
    const g = ctx.createLinearGradient(0, 0, 0, hh);
    g.addColorStop(0, "#15131c"); g.addColorStop(1, "#0b0a0f");
    ctx.fillStyle = g; ctx.fillRect(0, 0, w, hh);
    draw(ctx, w, hh);
  };
  requestAnimationFrame(() => cv.redraw());
  new ResizeObserver(() => cv.redraw()).observe(cv);
  return cv;
}
const glow = (ctx, color, blur = 10) => { ctx.shadowColor = color; ctx.shadowBlur = blur; };
const noGlow = (ctx) => { ctx.shadowBlur = 0; };

const PK_DISPLAY = {
  // the EQ: the curve, with a handle per band to drag (frequency and gain; scroll for Q)
  eq(slot, color, setParam, gesture) {
    const sp = (k) => pkSpec("eq", k);
    const RANGE = 18;
    const bands = [
      { f: "lowcut", g: null, label: "LC" }, { f: "lowfreq", g: "low", label: "L" },
      { f: "midfreq", g: "mid", label: "M", q: "q" }, { f: "highfreq", g: "high", label: "H" }, { f: "highcut", g: null, label: "HC" },
    ];
    let drag = null, hover = null;
    const geo = (w, h) => ({ x: (f) => Math.log(f / 20) / Math.log(1000) * w, fx: (px) => 20 * 1000 ** (px / w), y: (db) => h / 2 - db / RANGE * (h / 2 - 10), dby: (py) => (h / 2 - py) / (h / 2 - 10) * RANGE });
    const pos = (b, w, h) => {
      const G = geo(w, h), v = slot[b.f] ?? FX_DEFAULTS.eq[b.f];
      let f = pkReal(sp(b.f), v);
      if (b.f === "lowcut" && v < 0.005) f = 20; if (b.f === "highcut" && v >= 0.995) f = 20000;
      const db = b.g ? pkReal(sp(b.g), slot[b.g] ?? 0.5) : 0;
      return [Math.max(9, Math.min(w - 9, G.x(f))), G.y(db)];
    };
    const cv = pkCanvas("eqd", (ctx, w, h) => {
      const G = geo(w, h);
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (const f of [50, 100, 200, 500, 1000, 2000, 5000, 10000]) ctx.fillRect(Math.round(G.x(f)), 0, 1, h);
      for (const db of [-12, -6, 0, 6, 12]) ctx.fillRect(0, Math.round(G.y(db)), w, 1);
      ctx.fillStyle = "rgba(255,255,255,0.35)"; ctx.font = "600 9px system-ui, sans-serif";
      for (const [f, l] of [[100, "100"], [1000, "1k"], [10000, "10k"]]) ctx.fillText(l, G.x(f) + 3, h - 4);
      for (const db of [-12, 12]) ctx.fillText((db > 0 ? "+" : "") + db, 3, G.y(db) - 2);
      const coefs = eqCoefs(Object.fromEntries(PK_SPEC.eq.map(([k]) => [k, slot[k] ?? FX_DEFAULTS.eq[k]])), ui.rate || 48000);
      ctx.beginPath();
      for (let px = 0; px <= w; px++) { const db = Math.max(-RANGE - 4, Math.min(RANGE + 4, eqDb(coefs, G.fx(px), ui.rate || 48000))); px ? ctx.lineTo(px, G.y(db)) : ctx.moveTo(px, G.y(db)); }
      const fill = ctx.createLinearGradient(0, 0, 0, h);
      fill.addColorStop(0, color + "55"); fill.addColorStop(1, color + "08");
      ctx.save(); ctx.lineTo(w, G.y(0)); ctx.lineTo(0, G.y(0)); ctx.closePath(); ctx.fillStyle = fill; ctx.fill(); ctx.restore();
      ctx.beginPath();
      for (let px = 0; px <= w; px++) { const db = Math.max(-RANGE - 4, Math.min(RANGE + 4, eqDb(coefs, G.fx(px), ui.rate || 48000))); px ? ctx.lineTo(px, G.y(db)) : ctx.moveTo(px, G.y(db)); }
      glow(ctx, color, 12); ctx.strokeStyle = color; ctx.lineWidth = 2.2; ctx.stroke(); noGlow(ctx);
      for (const b of bands) {
        const [x, y] = pos(b, w, h), on = hover === b || drag?.b === b;
        ctx.beginPath(); ctx.arc(x, y, on ? 8 : 6.5, 0, 2 * Math.PI);
        ctx.fillStyle = on ? "#fff" : color; glow(ctx, color, on ? 14 : 6); ctx.fill(); noGlow(ctx);
        ctx.fillStyle = on ? "#16141c" : "#0b0a0f"; ctx.font = "800 8.5px system-ui, sans-serif"; ctx.textAlign = "center";
        ctx.fillText(b.label, x, y + 3); ctx.textAlign = "left";
      }
      if (hover || drag) {
        const b = drag?.b || hover, [x, y] = pos(b, w, h);
        const txt = pkFmt(sp(b.f), slot[b.f] ?? FX_DEFAULTS.eq[b.f]) + (b.g ? "  " + pkFmt(sp(b.g), slot[b.g] ?? 0.5) : "") + (b.q ? "  Q " + pkFmt(sp("q"), slot.q ?? FX_DEFAULTS.eq.q) : "");
        ctx.font = "700 10.5px system-ui, sans-serif";
        const tw = ctx.measureText(txt).width + 12, tx = Math.min(w - tw - 2, Math.max(2, x - tw / 2)), ty = y < 30 ? y + 14 : y - 26;
        ctx.fillStyle = "rgba(10,9,14,0.88)"; ctx.fillRect(tx, ty, tw, 18);
        ctx.fillStyle = "#fff"; ctx.fillText(txt, tx + 6, ty + 13);
      }
    }, 170);
    const hit = (e) => {
      const r = cv.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top;
      let best = null, bd = 14;
      for (const b of bands) { const [x, y] = pos(b, r.width, r.height), d = Math.hypot(x - px, y - py); if (d < bd) { bd = d; best = b; } }
      return { b: best, px, py, w: r.width, h: r.height };
    };
    cv.addEventListener("pointermove", (e) => {
      if (drag) return;
      const hb = hit(e).b;
      if (hb !== hover) { hover = hb; cv.style.cursor = hb ? "grab" : "default"; cv.redraw(); }
    });
    cv.addEventListener("pointerleave", () => { hover = null; cv.redraw(); });
    cv.addEventListener("pointerdown", (e) => {
      const hh = hit(e);
      if (!hh.b) return;
      cv.setPointerCapture(e.pointerId);
      gesture();
      drag = { b: hh.b };
      const move = (ev) => {
        const q = hit(ev), G = geo(q.w, q.h), b = drag.b;
        const fq = G.fx(Math.max(0, Math.min(q.w, q.px)));
        setParam(b.f, pkNorm(sp(b.f), fq));
        if (b.g && !ev.shiftKey) setParam(b.g, pkNorm(sp(b.g), Math.max(-15, Math.min(15, G.dby(q.py)))));
        cv.redraw();
      };
      const up = () => { drag = null; cv.removeEventListener("pointermove", move); cv.removeEventListener("pointerup", up); cv.redraw(); };
      cv.addEventListener("pointermove", move); cv.addEventListener("pointerup", up);
    });
    cv.addEventListener("dblclick", (e) => { const hh = hit(e); if (!hh.b) return; gesture(); setParam(hh.b.f, FX_DEFAULTS.eq[hh.b.f]); if (hh.b.g) setParam(hh.b.g, 0.5); });
    cv.addEventListener("wheel", (e) => {
      const hh = hit(e);
      if (!hh.b || !hh.b.q) return;
      e.preventDefault(); gesture();
      setParam("q", Math.max(0, Math.min(1, (slot.q ?? FX_DEFAULTS.eq.q) - Math.sign(e.deltaY) * 0.03)));
    }, { passive: false });
    cv.title = "Drag a handle: frequency and gain (Shift: frequency only). Scroll on M: its width (Q). Double-click: back to flat.";
    return cv;
  },

  // the compressor: input level against output level, the signal as a dot on it, and the
  // gain reduction over the last seconds
  comp(slot, color) {
    const hist = [];
    let lastIn = 0;
    const cv = pkCanvas("compd", (ctx, w, h) => {
      const g = (k) => pkReal(pkSpec("comp", k), slot[k] ?? FX_DEFAULTS.comp[k]);
      const thr = g("threshold"), ratio = g("ratio"), mk = g("makeup");
      const sz = h - 16, ox = 8, oy = 8, lo = -60, hi = 0;
      const X = (db) => ox + (db - lo) / (hi - lo) * sz, Y = (db) => oy + sz - (Math.max(lo, Math.min(hi + 18, db)) - lo) / (hi - lo) * sz;
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (let d = -60; d <= 0; d += 12) { ctx.fillRect(Math.round(X(d)), oy, 1, sz); ctx.fillRect(ox, Math.round(Y(d)), sz, 1); }
      ctx.strokeStyle = "rgba(255,255,255,0.12)"; ctx.beginPath(); ctx.moveTo(X(lo), Y(lo)); ctx.lineTo(X(hi), Y(hi)); ctx.stroke();
      const out = (i) => (i <= thr ? i : thr + (i - thr) / ratio) + mk;
      ctx.beginPath();
      for (let i = lo; i <= hi; i += 0.5) i === lo ? ctx.moveTo(X(i), Y(out(i))) : ctx.lineTo(X(i), Y(out(i)));
      glow(ctx, color, 10); ctx.strokeStyle = color; ctx.lineWidth = 2.2; ctx.stroke(); noGlow(ctx);
      ctx.fillStyle = color + "aa"; ctx.fillRect(X(thr) - 0.5, oy, 1, sz);
      // the signal now
      const inDb = lastIn > 1e-6 ? 20 * Math.log10(lastIn) : lo;
      if (inDb > lo) { ctx.beginPath(); ctx.arc(X(Math.min(hi, inDb)), Y(out(Math.min(hi, inDb))), 5, 0, 2 * Math.PI); ctx.fillStyle = "#fff"; glow(ctx, "#fff", 12); ctx.fill(); noGlow(ctx); }
      // gain reduction history, to the right
      const hx = ox + sz + 14, hw = w - hx - 6;
      if (hw > 30) {
        ctx.fillStyle = "rgba(255,255,255,0.04)"; ctx.fillRect(hx, oy, hw, sz);
        ctx.beginPath(); ctx.moveTo(hx, oy);
        const HX = (k) => hx + hw - (hist.length - 1 - k) / 199 * hw;
        ctx.moveTo(HX(0), oy);
        hist.forEach((gr, k) => ctx.lineTo(HX(k), oy + Math.min(sz, gr / 24 * sz)));
        ctx.lineTo(hx + hw, oy); ctx.closePath();
        ctx.fillStyle = "#ff7a6b66"; ctx.fill();
        ctx.fillStyle = "rgba(255,255,255,0.45)"; ctx.font = "600 9px system-ui, sans-serif";
        ctx.fillText("Gain reduction", hx + 4, oy + sz - 4);
        ctx.fillText(`−${(hist[hist.length - 1] || 0).toFixed(1)} dB`, hx + hw - 44, oy + 11);
      }
      ctx.fillStyle = "rgba(255,255,255,0.45)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText(`${thr.toFixed(0)} dB · ${ratio.toFixed(1)}:1`, ox + 4, oy + 11);
    }, 170);
    cv.live = (m) => { lastIn = m[0]; hist.push(m[2]); if (hist.length > 200) hist.shift(); cv.redraw(); };
    return cv;
  },

  // the limiter: gain reduction over time under the ceiling
  limiter(slot, color) {
    const hist = [], outs = [];
    const cv = pkCanvas("limd", (ctx, w, h) => {
      const ceil = pkReal(pkSpec("limiter", "ceiling"), slot.ceiling ?? FX_DEFAULTS.limiter.ceiling);
      const Y = (db) => 8 + (-db) / 24 * (h - 16);
      ctx.fillStyle = "rgba(255,255,255,0.05)";
      for (let d = 0; d >= -24; d -= 6) ctx.fillRect(0, Math.round(Y(d)), w, 1);
      // the output level as it went
      const HX = (k) => w - (outs.length - 1 - k) / 199 * w;
      ctx.beginPath(); ctx.moveTo(HX(0), h);
      outs.forEach((o, k) => ctx.lineTo(HX(k), Y(Math.max(-24, o > 1e-6 ? 20 * Math.log10(o) : -24))));
      ctx.lineTo(w, h); ctx.closePath();
      ctx.fillStyle = color + "33"; ctx.fill();
      // and how much was taken off, from the top
      ctx.beginPath(); ctx.moveTo(HX(0), Y(0));
      hist.forEach((gr, k) => ctx.lineTo(HX(k), Y(-Math.min(24, gr))));
      glow(ctx, "#ff7a6b", 8); ctx.strokeStyle = "#ff7a6b"; ctx.lineWidth = 2; ctx.stroke(); noGlow(ctx);
      ctx.fillStyle = color; ctx.fillRect(0, Math.round(Y(ceil)), w, 1.5);
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText(`Ceiling ${ceil.toFixed(1)} dB`, 6, Y(ceil) - 3);
      ctx.fillText(`−${(hist[hist.length - 1] || 0).toFixed(1)} dB`, w - 44, 14);
    }, 150);
    cv.live = (m) => { hist.push(m[2]); outs.push(m[1]); if (hist.length > 200) { hist.shift(); outs.shift(); } cv.redraw(); };
    return cv;
  },

  // the delay: the echoes as they come, left and right
  delay(slot, color) {
    return pkCanvas("dlyd", (ctx, w, h) => {
      const g = (k) => slot[k] ?? FX_DEFAULTS.delay[k];
      const ms = pkReal(pkSpec("delay", "time"), g("time")), fb = g("feedback"), pp = g("pingpong"), mix = g("mix");
      const span = Math.max(2000, ms * 6), X = (t) => 10 + t / span * (w - 20), mid = h / 2;
      ctx.fillStyle = "rgba(255,255,255,0.06)"; ctx.fillRect(0, mid, w, 1);
      ctx.fillStyle = "rgba(255,255,255,0.4)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText("L", 4, 14); ctx.fillText("R", 4, h - 6);
      const full = h / 2 - 12, dry = g("dry");
      ctx.fillStyle = "#fff"; ctx.fillRect(X(0) - 1.5, mid - full * dry, 3, 2 * full * dry);
      // each echo: the left and right halves, ping-pong moves them from side to side
      let a = 0.3 + 0.7 * mix;
      const env = [];
      for (let k = 1; ms * k < span && a > 0.01; k++, a *= fb) {
        const x = X(ms * k), toL = k % 2 ? 1 : 1 - pp, toR = k % 2 ? 1 - pp : 1;
        env.push([x, a]);
        glow(ctx, color, 10); ctx.fillStyle = color;
        ctx.fillRect(x - 2.5, mid - full * a * toL, 5, full * a * toL);
        ctx.fillStyle = color + "aa";
        ctx.fillRect(x - 2.5, mid, 5, full * a * toR);
        noGlow(ctx);
      }
      if (env.length > 1) {
        ctx.beginPath(); env.forEach(([x, a], k) => (k ? ctx.lineTo(x, mid - full * a) : ctx.moveTo(x, mid - full * a)));
        ctx.setLineDash([3, 4]); ctx.strokeStyle = color + "88"; ctx.lineWidth = 1; ctx.stroke(); ctx.setLineDash([]);
      }
      ctx.fillStyle = "rgba(255,255,255,0.5)";
      ctx.fillText(`${Math.round(ms)} ms · ${Math.round(fb * 100)}% feedback`, w - 150, 14);
    }, 130);
  },

  // the reverb: early reflections, then the tail as it fades (darker with damping)
  reverb(slot, color) {
    return pkCanvas("revd", (ctx, w, h) => {
      const g = (k) => slot[k] ?? FX_DEFAULTS.reverb[k];
      const size = g("size"), damp = g("damp"), width = g("width"), mix = g("mix");
      const len = 0.25 + size * 0.75, base = h - 12;
      const grad = ctx.createLinearGradient(0, 0, w, 0);
      grad.addColorStop(0, color + "cc"); grad.addColorStop(1, damp > 0.5 ? "#30264066" : color + "22");
      ctx.beginPath(); ctx.moveTo(12, base);
      for (let x = 12; x <= w - 8; x++) {
        const t = (x - 12) / (w - 20), env = Math.exp(-t / len * 3.2) * mix * 1.6;
        const jitter = 0.75 + 0.25 * Math.sin(x * 1.7) * Math.sin(x * 0.31);
        ctx.lineTo(x, base - Math.min(1, env * jitter) * (h - 24));
      }
      ctx.lineTo(w - 8, base); ctx.closePath(); ctx.fillStyle = grad; ctx.fill();
      for (let k = 0; k < 9; k++) { // early reflections
        const t = 0.01 + k * 0.012 * (0.5 + size), x = 12 + t * (w - 20), a = (1 - k / 10) * mix * 1.4;
        ctx.fillStyle = "#fff"; ctx.fillRect(x, base - Math.min(1, a) * (h - 24), 2, Math.min(1, a) * (h - 24));
      }
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
      ctx.fillText(`${(0.3 + size * 4.7).toFixed(1)} s tail · width ${Math.round(width * 100)}%`, w - 160, 14);
    }, 130);
  },

  // drive: the shape the sound is bent into
  drive(slot, color) {
    return pkCanvas("drvd", (ctx, w, h) => {
      const d = slot.drive ?? 0.4, k = 1 + 40 * d * d, sz = h - 16, ox = (w - sz) / 2, oy = 8, nrm = 1 / Math.sqrt(Math.tanh(k));
      ctx.fillStyle = "rgba(255,255,255,0.05)"; ctx.fillRect(ox + sz / 2, oy, 1, sz); ctx.fillRect(ox, oy + sz / 2, sz, 1);
      ctx.strokeStyle = "rgba(255,255,255,0.12)"; ctx.beginPath(); ctx.moveTo(ox, oy + sz); ctx.lineTo(ox + sz, oy); ctx.stroke();
      ctx.beginPath();
      for (let i = 0; i <= 100; i++) { const x = i / 50 - 1, y = Math.max(-1.15, Math.min(1.15, Math.tanh(k * x) * nrm / 1.15)); i ? ctx.lineTo(ox + (x + 1) / 2 * sz, oy + (1 - y) / 2 * sz) : ctx.moveTo(ox, oy + (1 - y) / 2 * sz); }
      glow(ctx, color, 10); ctx.strokeStyle = color; ctx.lineWidth = 2.4; ctx.stroke(); noGlow(ctx);
    }, 130);
  },

  // the amp: its model (load, swap, what it is)
  nam(slot, color, setParam, gesture, t, pos) { const d = namSection(t, pos, slot); d.classList.add("pkdisp", "pknam"); return d; },

  // chorus and tremolo: their LFO, moving
  chorus(slot, color) { return pkLfo(slot, color, "chorus"); },
  tremolo(slot, color) { return pkLfo(slot, color, "tremolo"); },
};

function pkLfo(slot, color, kind) {
  const t0 = performance.now();
  const cv = pkCanvas("lfod", (ctx, w, h) => {
    const rate = pkReal(pkSpec(kind, "rate"), slot.rate ?? FX_DEFAULTS[kind].rate), depth = slot.depth ?? 0.5;
    const ph = (performance.now() - t0) / 1000 * rate * 2 * Math.PI, mid = h / 2;
    ctx.fillStyle = "rgba(255,255,255,0.06)"; ctx.fillRect(0, mid, w, 1);
    for (const [side, c] of [[0, color], [kind === "tremolo" ? Math.PI * (slot.stereo ?? 0) : Math.PI / 2, "#ffffff70"]]) {
      ctx.beginPath();
      for (let x = 0; x <= w; x++) { const y = mid - Math.sin(x / w * 4 * Math.PI - ph + side) * depth * (h / 2 - 10); x ? ctx.lineTo(x, y) : ctx.moveTo(x, y); }
      glow(ctx, c, 8); ctx.strokeStyle = c; ctx.lineWidth = 2; ctx.stroke(); noGlow(ctx);
    }
    ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = "600 9px system-ui, sans-serif";
    ctx.fillText(`${rate.toFixed(2)} Hz`, 6, 14);
  }, 110);
  cv.live = () => cv.redraw();
  return cv;
}
