"""Measured partials against the model's modes.

    python tools/compare.py measured.json model.json [--wav-a rec.wav --wav-b model.wav]
                            [--out compare.png] [--cents 50]

Both JSON files are in the analyze.py / mode_table --json format. Prints a table that pairs every
measured partial with the model's nearest mode (within `--cents`), and draws: frequency against T60
for both (the dot's size is its level), and, if WAVs are given, their spectrograms side by side.
Needs numpy, scipy, matplotlib.
"""
from __future__ import annotations

import argparse
import json
import math
import sys

import numpy as np


def load(path):
    with open(path) as f:
        d = json.load(f)
    return d.get("source", path), d["partials"]


def match(measured, model, cents=50.0):
    """For each measured partial: the model mode nearest in pitch, if within `cents`."""
    rows = []
    for p in sorted(measured, key=lambda p: -p.get("amp", 0)):
        best, bc = None, None
        for q in model:
            c = 1200 * math.log2(q["freq"] / p["freq"])
            if abs(c) <= cents and (bc is None or abs(c) < abs(bc)):
                best, bc = q, c
        rows.append((p, best, bc))
    return rows


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("measured")
    ap.add_argument("model")
    ap.add_argument("--wav-a")
    ap.add_argument("--wav-b")
    ap.add_argument("--out", default="compare.png")
    ap.add_argument("--cents", type=float, default=50.0)
    ap.add_argument("--min-amp", type=float, default=0.0, help="leave out model modes weaker than this (linear)")
    a = ap.parse_args(argv)
    src_a, meas = load(a.measured)
    src_b, model = load(a.model)
    model = [q for q in model if q.get("amp", 1) >= a.min_amp]
    rows = match(meas, model, a.cents)
    print(f"{'f meas':>9} {'f model':>9} {'cents':>7} {'T60 meas':>9} {'T60 model':>9} {'ratio':>6}  model mode")
    dc, dt = [], []
    for p, q, c in rows:
        if q is None:
            print(f"{p['freq']:9.1f} {'-':>9} {'-':>7} {p['t60']:9.3f} {'-':>9} {'-':>6}")
            continue
        r = q["t60"] / p["t60"] if p["t60"] > 0 and math.isfinite(p["t60"]) else float("nan")
        print(f"{p['freq']:9.1f} {q['freq']:9.1f} {c:7.1f} {p['t60']:9.3f} {q['t60']:9.3f} {r:6.2f}  {q.get('label', '')}")
        dc.append(abs(c))
        if math.isfinite(r):
            dt.append(abs(math.log2(r)))
    matched = sum(q is not None for _, q, _ in rows)
    print(f"\n{matched} of {len(rows)} measured partials have a model mode within {a.cents:.0f} cents", file=sys.stderr)
    if dc:
        print(f"median |pitch error| {np.median(dc):.1f} cents; median T60 error x{2 ** np.median(dt):.2f}", file=sys.stderr)

    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    spect = a.wav_a and a.wav_b
    fig, axes = plt.subplots(1, 3 if spect else 1, figsize=(16 if spect else 7, 5), squeeze=False)
    ax = axes[0][0]
    for pts, label, color in ((meas, f"measured ({src_a})", "tab:blue"), (model, f"model ({src_b})", "tab:orange")):
        f = [p["freq"] for p in pts if math.isfinite(p["t60"])]
        t = [p["t60"] for p in pts if math.isfinite(p["t60"])]
        s = [8 + 120 * min(1.0, p.get("amp", 0.5)) for p in pts if math.isfinite(p["t60"])]
        ax.scatter(f, t, s=s, alpha=0.6, label=label, color=color)
    ax.set_xscale("log")
    ax.set_yscale("log")
    ax.set_xlabel("frequency (Hz)")
    ax.set_ylabel("T60 (s)")
    ax.legend(fontsize=8)
    ax.grid(True, which="both", alpha=0.3)
    if spect:
        from analyze import read_wav
        for ax, path in zip(axes[0][1:], (a.wav_a, a.wav_b)):
            fs, x = read_wav(path)
            ax.specgram(x, NFFT=4096, Fs=fs, noverlap=3584, cmap="magma", vmin=-140)
            ax.set_ylim(0, 3000)
            ax.set_title(path, fontsize=9)
            ax.set_xlabel("time (s)")
            ax.set_ylabel("Hz")
    fig.tight_layout()
    fig.savefig(a.out, dpi=110)
    print(f"-> {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
