// Duet — the Master window: the master's loudness the way streaming services and broadcasters
// measure it (EBU R128: momentary, short-term, integrated LUFS, loudness range, true peak), a
// minute of short-term history against a target, the spectrum and the stereo correlation.

const ms = { target: -14, hist: [], last: 0 };

function masterSetup() {
  ms.target = +store.get("lufsTarget", -14);
  const seg = $("msTarget");
  if (!seg) return;
  const mark = () => { for (const b of seg.querySelectorAll("button")) b.classList.toggle("on", +b.dataset.v === ms.target); };
  for (const b of seg.querySelectorAll("button")) b.onclick = () => { ms.target = +b.dataset.v; store.set("lufsTarget", ms.target); mark(); };
  mark();
  $("msFx").onclick = () => { setMainView("mixer"); selectStrip?.("M"); };
  $("msExport").onclick = () => { exp.norm = String(ms.target); openExport(); };
}

const fmtL = (x) => (x == null ? "–" : x.toFixed(1).replace("-", "−"));

function masterPaint(s) {
  if (!winShown("master") || !$("msS")) return;
  const now = performance.now();
  // a minute of history, ten points a second
  if (now - ms.last > 100) {
    ms.last = now;
    ms.hist.push(s.lufs_s ?? null);
    if (ms.hist.length > 600) ms.hist.shift();
  }
  $("msS").textContent = fmtL(s.lufs_s);
  $("msI").textContent = fmtL(s.lufs_i);
  $("msM").textContent = fmtL(s.lufs_m);
  $("msLRA").textContent = s.lufs_range == null ? "–" : s.lufs_range.toFixed(1) + " LU";
  $("msTP").textContent = s.true_peak == null ? "–" : fmtL(s.true_peak) + " dBTP";
  $("msTP").classList.toggle("bad", s.true_peak != null && s.true_peak > -1);
  $("msMax").textContent = fmtL(s.lufs_max_s);
  const ref = s.lufs_i ?? s.lufs_s;
  $("msVerdict").textContent = ref == null ? "Play the song to measure it."
    : Math.abs(ref - ms.target) < 1 ? "On target." : `${Math.abs(ref - ms.target).toFixed(1)} LU ${ref > ms.target ? "louder" : "quieter"} than the target.`
      + (s.true_peak != null && s.true_peak > -1 ? " True peak over −1 dBTP: a limiter on the master helps." : "");
  histDraw();
  specDraw(s.spectrum || []);
  const c = s.correlation ?? 1;
  const bar = $("msCorr");
  bar.style.left = `${Math.min(50, (c + 1) * 50)}%`;
  bar.style.width = `${Math.abs(c) * 50}%`;
  bar.style.background = c < 0 ? "var(--bad)" : c < 0.3 ? "var(--warn)" : "var(--ok)";
  $("msCorrV").textContent = `${c >= 0 ? "+" : "−"}${Math.abs(c).toFixed(2)} ${c > 0.9 ? "(near mono)" : c < 0 ? "(out of phase!)" : "(wide)"}`;
}

function histDraw() {
  const cv = $("msHist"), w = cv.clientWidth, h = cv.clientHeight;
  if (!w || !h) return;
  const ctx = sizeCanvas(cv, w, h, false);
  ctx.fillStyle = "#0e0d12"; ctx.fillRect(0, 0, w, h);
  const lo = -40, hi = -3, y = (v) => h - (Math.max(lo, Math.min(hi, v)) - lo) / (hi - lo) * h;
  ctx.font = "10px system-ui, sans-serif";
  for (const v of [-30, -20, -14, -9]) {
    ctx.fillStyle = "#2a2632"; ctx.fillRect(0, Math.round(y(v)), w, 1);
    ctx.fillStyle = "#6c6577"; ctx.fillText(String(v).replace("-", "−"), 4, y(v) - 2);
  }
  ctx.strokeStyle = "#ffd166"; ctx.setLineDash([4, 4]);
  ctx.beginPath(); ctx.moveTo(0, y(ms.target)); ctx.lineTo(w, y(ms.target)); ctx.stroke();
  ctx.setLineDash([]);
  ctx.strokeStyle = "#4fd6c6"; ctx.lineWidth = 1.5;
  ctx.beginPath();
  let on = false;
  ms.hist.forEach((v, i) => {
    const x = w - (ms.hist.length - 1 - i) * (w / 600);
    if (v == null) { on = false; return; }
    if (on) ctx.lineTo(x, y(v)); else ctx.moveTo(x, y(v));
    on = true;
  });
  ctx.stroke();
  ctx.lineWidth = 1;
}

function specDraw(bands) {
  const cv = $("msSpec"), w = cv.clientWidth, h = cv.clientHeight;
  if (!w || !h) return;
  const ctx = sizeCanvas(cv, w, h, false);
  ctx.fillStyle = "#0e0d12"; ctx.fillRect(0, 0, w, h);
  const n = bands.length || 31, bw = w / n, lo = -84, hi = 0;
  for (let i = 0; i < n; i++) {
    const v = bands[i] ?? lo, frac = (Math.max(lo, Math.min(hi, v)) - lo) / (hi - lo);
    ctx.fillStyle = `hsl(${180 - frac * 150}, 70%, ${45 + frac * 15}%)`;
    ctx.fillRect(i * bw + 1, h - frac * (h - 12), bw - 2, frac * (h - 12));
  }
  ctx.fillStyle = "#6c6577";
  ctx.font = "10px system-ui, sans-serif";
  for (const [hz, label] of [[100, "100"], [1000, "1k"], [10000, "10k"]]) {
    const i = Math.log(hz / 20) / Math.log(1000) * n;
    ctx.fillText(label, i * bw, 10);
  }
}
