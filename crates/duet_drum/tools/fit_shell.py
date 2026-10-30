import json, math, subprocess, sys
sys.path.insert(0, "tools")
import analyze
D = sys.argv[1]
# the recurring partials of the recording: freq, T60, level re the fundamental at the centre, at the edge
TARGET = [(534.3, 1.21, -29.1, -13.0), (660.9, 1.05, -28.8, -20.8), (684.6, 0.98, -27.0, -22.1), (731.1, 0.88, -33.9, -18.2),
          (794.1, 0.88, -34.1, -20.4), (917.9, 0.66, -26.8, None), (1000.2, 0.62, -29.9, -12.8), (1068.5, 0.46, -26.1, -10.1), (1688.2, 0.36, -12.2, -5.5)]
base = json.load(open("presets/pearl16.json"))

def levels(params, vel, r, seconds):
    open(f"{D}/fit.json", "w").write(json.dumps(params))
    subprocess.run(["target/release/examples/render", "--params", f"{D}/fit.json", "--hit", f"{vel},{r},0", "--seconds", str(seconds), "--fs", "44100", "--out", f"{D}/fit"], capture_output=True, check=True)
    fs, x = analyze.read_wav(f"{D}/fit_close.wav")
    parts = analyze.analyze(x, fs, max_partials=80)
    fund = max((p for p in parts if p["freq"] < 90), key=lambda p: p["amp"])
    out = []
    for f, *_ in TARGET:
        near = [p for p in parts if abs(p["freq"] / f - 1) < 0.01]
        out.append(20 * math.log10(max(near, key=lambda p: p["amp"])["amp"] / fund["amp"]) if near else None)
    return out

gains = [1.0] * len(TARGET)
boost = float(sys.argv[2]) if len(sys.argv) > 2 else 4.0
for it in range(4):
    p = dict(base, shell=[{"freq": f, "t60": t, "gain": g} for (f, t, _, _), g in zip(TARGET, gains)], shell_edge_boost=boost)
    lv = levels(p, 0.7, 0.05, 2.3)
    for i, (l, (_, _, c, _)) in enumerate(zip(lv, TARGET)):
        if l is not None:
            gains[i] *= 10 ** ((c - l) / 20)
        else:
            gains[i] *= 10.0
    print(f"iteration {it}: centre levels", [None if l is None else round(l, 1) for l in lv], file=sys.stderr)
p = dict(base, shell=[{"freq": f, "t60": t, "gain": round(g, 6)} for (f, t, _, _), g in zip(TARGET, gains)], shell_edge_boost=boost)
lc = levels(p, 0.7, 0.05, 2.3)
le = levels(p, 1.0, 0.85, 1.8)
print("freq   centre: model/target   edge: model/target", file=sys.stderr)
for (f, _, c, e), a, b in zip(TARGET, lc, le):
    print(f"{f:7.1f}   {a if a is None else round(a,1)!s:>6} / {c:6.1f}     {b if b is None else round(b,1)!s:>6} / {e!s:>6}", file=sys.stderr)
print(json.dumps(p, indent=2))
