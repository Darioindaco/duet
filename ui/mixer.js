"use strict";
// Duet: the song's mixer beyond plain strips. Buses (mixer tracks that other
// tracks feed: a shared reverb, a drum bus), where each track goes (a bus or
// the master) and its sends, a compressor's sidechain, the EQ's curve. All of
// it is part of the shared song, like a track's effects.
//
// Loaded before the main script; the main script's helpers exist by the time
// anything here runs.

const BUS_COLOR = "#c9a3ff";

const busesOf = () => (song.view ? song.view.buses || [] : []);
const allTracks = () => (song.view ? [...song.view.tracks, ...(song.view.buses || [])] : []);
const trackById = (id) => allTracks().find((t) => t.id === id);

// where a track's sound goes: its bus (if that's a bus) and its sends
function routesOf(t) {
  const out = [];
  if (t.out && trackById(t.out)?.bus) out.push(t.out);
  for (const [b, v] of Object.entries(t.sends || {})) if (v > 0 && trackById(b)?.bus) out.push(b);
  return out;
}

// does `from`'s sound end up in `to`? (a bus can't go to one that feeds it)
function reaches(from, to) {
  const seen = new Set(), stack = [from];
  while (stack.length) {
    const x = stack.pop();
    if (x === to) return true;
    if (seen.has(x)) continue;
    seen.add(x);
    const t = trackById(x);
    if (t) stack.push(...routesOf(t));
  }
  return false;
}

const feedersOf = (bus) => allTracks().filter((t) => t.id !== bus.id && routesOf(t).includes(bus.id));

function addBusMenu(e) {
  const r = e.currentTarget.getBoundingClientRect();
  showCtx([
    ["Reverb bus", "", () => addBus("reverb")],
    ["Delay bus", "", () => addBus("delay")],
    ["Empty bus (e.g. a drum bus)", "", () => addBus("")],
  ], r.left, r.bottom + 4);
}

function addBus(kind) {
  const v = song.view, buses = busesOf();
  const order = 1000 + buses.reduce((a, b) => Math.max(a, (b.order || 1000) - 1000), 0) + 1;
  const name = kind === "reverb" ? "Reverb" : kind === "delay" ? "Delay" : `Bus ${buses.length + 1}`;
  // a send bus is all effect: the dry sound is already there from the track itself
  const rack = kind === "reverb" ? [{ kind: "reverb", on: true, ...FX_DEFAULTS.reverb, size: 0.75, mix: 0.5, dry: 0 }]
    : kind === "delay" ? [{ kind: "delay", on: true, ...FX_DEFAULTS.delay, feedback: 0.45, mix: 1, pingpong: 0.6, dry: 0 }] : [];
  songEdit([["track", newId("b"), { name, order, owner: v.me, bus: true, vol: 0.8, pan: 0, rack }]]);
  toast(kind ? `${name} bus added: turn up its send on a track (the Routing box in the track's FX rack).`
    : "Bus added: send tracks to it with Output in their Routing box.");
}

function deleteBus(b) {
  const n = feedersOf(b).length;
  if (n && !confirm(`Delete "${b.name}"? ${n} track${n === 1 ? "" : "s"} going to it go${n === 1 ? "es" : ""} to the master again.`)) return;
  const ch = [["track", b.id, { del: true }]];
  for (const t of allTracks()) {
    if (t.out === b.id) ch.push(["track", t.id, { out: null }]);
    if (t.sends && b.id in t.sends) {
      const s = { ...t.sends };
      delete s[b.id];
      ch.push(["track", t.id, { sends: s }]);
    }
  }
  songEdit(ch);
}

// the routing box at the start of a song track's (or bus's) rack
function routingCard(track) {
  const p = document.createElement("div");
  p.className = "pedal routep";
  p.style.setProperty("--c", BUS_COLOR);
  p.innerHTML = `<div class="top"><div class="ttl"><div class="name">Routing</div><div class="sub">where it goes</div></div></div>`;
  const row = document.createElement("label");
  row.className = "rrow";
  row.innerHTML = "<span>Output</span>";
  const sel = document.createElement("select");
  sel.append(new Option("Master", ""));
  const ok = (b) => b.id !== track.id && !reaches(b.id, track.id);
  for (const b of busesOf().filter(ok)) sel.append(new Option(b.name || "Bus", b.id, false, b.id === track.out));
  if (!track.out || ![...sel.options].some((o) => o.value === track.out)) sel.value = "";
  sel.title = "Where this track's sound goes after its fader: the master, or a bus (a group: e.g. all drums through one compressor)";
  sel.onchange = () => songEdit([["track", track.id, { out: sel.value || null }]]);
  row.append(sel);
  p.append(row);
  const targets = busesOf().filter((b) => ok(b) && b.id !== track.out);
  if (targets.length) {
    p.insertAdjacentHTML("beforeend", `<div class="rlbl">Sends <span>to buses, after the fader</span></div>`);
    const knobs = document.createElement("div");
    knobs.className = "knobs";
    for (const b of targets) {
      knobs.append(knob({
        label: esc(b.name || "Bus"), value: (track.sends || {})[b.id] || 0, def: 0, color: BUS_COLOR,
        fmt: (v) => (v < 0.001 ? "off" : fmtDb(v)),
        onChange: (v) => {
          const s = { ...(track.sends || {}) };
          if (v < 0.001) delete s[b.id]; else s[b.id] = v;
          track.sends = s;
          throttledEdit("track", track.id, { sends: s });
        },
      }));
    }
    p.append(knobs);
    if (targets.length > 4) p.insertAdjacentHTML("beforeend", `<div class="mnote">Up to 4 sends per track are heard.</div>`);
  } else if (!track.bus) {
    p.insertAdjacentHTML("beforeend", `<div class="mnote">Sends go to buses: add a reverb or delay bus with <b>+ Bus</b> in the mixer, then turn up its knob here.</div>`);
  }
  return p;
}

// a compressor on a song track or the master can listen to another track
function keySelect(t, pos, slot) {
  const ks = document.createElement("select");
  ks.className = "keysel";
  ks.title = "Sidechain: the compressor reacts to another track instead of its own sound (a kick that pushes the bass down)";
  ks.append(new Option("Sidechain: off", ""));
  for (const tr of allTracks()) {
    if (tr.slot == null || "T" + tr.slot === t) continue;
    ks.append(new Option("Sidechain: " + (tr.name || "Track"), tr.slot, false, slot.key === tr.slot));
  }
  ks.onchange = async () => {
    await api.rack_set(t, pos, "key", ks.value === "" ? -1 : +ks.value);
    rackChanged();
  };
  return ks;
}

// ---------- the EQ's curve (the same filters as the engine's, fx.rs) ----------
function eqCoefs(p, sr) {
  const gain = (v) => -15 + 30 * v;
  const ny = sr * 0.45, TAU = 2 * Math.PI;
  const norm = (b, a) => [b[0] / a[0], b[1] / a[0], b[2] / a[0], a[1] / a[0], a[2] / a[0]];
  const hp = (f, q) => { const w = TAU * Math.min(f, ny) / sr, c = Math.cos(w), al = Math.sin(w) / (2 * q); return norm([(1 + c) / 2, -(1 + c), (1 + c) / 2], [1 + al, -2 * c, 1 - al]); };
  const lp = (f, q) => { const w = TAU * Math.min(f, ny) / sr, c = Math.cos(w), al = Math.sin(w) / (2 * q); return norm([(1 - c) / 2, 1 - c, (1 - c) / 2], [1 + al, -2 * c, 1 - al]); };
  const peak = (f, q, db) => {
    const A = 10 ** (db / 40), w = TAU * Math.min(f, ny) / sr, c = Math.cos(w), al = Math.sin(w) / (2 * q);
    return norm([1 + al * A, -2 * c, 1 - al * A], [1 + al / A, -2 * c, 1 - al / A]);
  };
  const shelf = (high, f, db) => {
    const A = 10 ** (db / 40), w = TAU * Math.min(f, ny) / sr, c = Math.cos(w), al = Math.sin(w) / 2 * Math.SQRT2, sq = 2 * Math.sqrt(A) * al;
    return high
      ? norm([A * ((A + 1) + (A - 1) * c + sq), -2 * A * ((A - 1) + (A + 1) * c), A * ((A + 1) + (A - 1) * c - sq)],
             [(A + 1) - (A - 1) * c + sq, 2 * ((A - 1) - (A + 1) * c), (A + 1) - (A - 1) * c - sq])
      : norm([A * ((A + 1) - (A - 1) * c + sq), 2 * A * ((A - 1) - (A + 1) * c), A * ((A + 1) - (A - 1) * c - sq)],
             [(A + 1) + (A - 1) * c + sq, -2 * ((A - 1) + (A + 1) * c), (A + 1) + (A - 1) * c - sq]);
  };
  const out = [];
  if (p.lowcut > 0.005) { const f = 20 * 40 ** p.lowcut; out.push(hp(f, 0.541), hp(f, 1.307)); }
  if (Math.abs(gain(p.low)) >= 0.05) out.push(shelf(false, 30 * 20 ** p.lowfreq, gain(p.low)));
  if (Math.abs(gain(p.mid)) >= 0.05) out.push(peak(100 * 100 ** p.midfreq, 0.3 * 33.3 ** p.q, gain(p.mid)));
  if (Math.abs(gain(p.high)) >= 0.05) out.push(shelf(true, 1500 * 10 ** p.highfreq, gain(p.high)));
  if (p.highcut < 0.995) out.push(lp(1000 * 20 ** p.highcut, Math.SQRT1_2));
  return out;
}

function eqDb(coefs, f, sr) {
  const w = 2 * Math.PI * f / sr;
  const c1 = Math.cos(w), s1 = Math.sin(w), c2 = Math.cos(2 * w), s2 = Math.sin(2 * w);
  let db = 0;
  for (const [b0, b1, b2, a1, a2] of coefs) {
    const num = (b0 + b1 * c1 + b2 * c2) ** 2 + (b1 * s1 + b2 * s2) ** 2;
    const den = (1 + a1 * c1 + a2 * c2) ** 2 + (a1 * s1 + a2 * s2) ** 2;
    db += 10 * Math.log10(num / den);
  }
  return db;
}

// a canvas that draws the slot's curve; .redraw() after a knob turns
function eqCurve(slot, color) {
  const cv = document.createElement("canvas");
  cv.className = "eqcv";
  cv.redraw = () => {
    const w = cv.clientWidth || 286, h = cv.clientHeight || 84;
    const ctx = sizeCanvas(cv, w, h);
    const sr = ui.rate || 48000, RANGE = 18;
    const x = (f) => Math.log(f / 20) / Math.log(1000) * w;
    const y = (db) => h / 2 - db / RANGE * (h / 2 - 5);
    ctx.fillStyle = "rgba(0,0,0,0.32)";
    ctx.fillRect(0, 0, w, h);
    ctx.fillStyle = "rgba(255,255,255,0.07)";
    for (const f of [100, 1000, 10000]) ctx.fillRect(Math.round(x(f)), 0, 1, h);
    for (const db of [-12, 0, 12]) ctx.fillRect(0, Math.round(y(db)), w, 1);
    ctx.fillStyle = "rgba(255,255,255,0.35)";
    ctx.font = "600 9px system-ui, sans-serif";
    ctx.fillText("100", x(100) + 3, h - 4); ctx.fillText("1k", x(1000) + 3, h - 4); ctx.fillText("10k", x(10000) + 3, h - 4);
    const coefs = eqCoefs(slot, sr);
    ctx.beginPath();
    for (let px = 0; px <= w; px++) {
      const f = 20 * 1000 ** (px / w);
      const db = Math.max(-RANGE - 4, Math.min(RANGE + 4, eqDb(coefs, f, sr)));
      px ? ctx.lineTo(px, y(db)) : ctx.moveTo(px, y(db));
    }
    ctx.strokeStyle = color;
    ctx.lineWidth = 2;
    ctx.stroke();
    ctx.lineTo(w, y(0)); ctx.lineTo(0, y(0)); ctx.closePath();
    ctx.fillStyle = color + "30";
    ctx.fill();
  };
  requestAnimationFrame(cv.redraw);
  return cv;
}
