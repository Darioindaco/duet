"""Partials of a drum hit's tail: their frequencies, decay times and levels, from a WAV.

    python tools/analyze.py hit.wav [--out hit.json] [--channel 0] [--max-partials 40]
                            [--fmin 40] [--fmax 3000] [--window 0.2] [--start-after 0.03]

A Blackman-Harris STFT over the tail (from `--start-after` seconds after the loudest sample), peaks picked in the
first frames, each peak followed frame by frame (it may glide), and its level in dB fitted with a
line until it nears the noise floor: T60 = -60 / slope. Writes JSON in the format the model exports
(`cargo run --example mode_table -- --json`): {source, fs, partials: [{freq, t60, amp, ...}]}, with
amp linear (the level extrapolated back to the onset) and the strongest partial at 1. Needs numpy and scipy.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from scipy.signal import find_peaks


def read_wav(path, channel=0):
    fs, x = wavfile.read(path)
    if x.ndim > 1:
        x = x[:, channel]
    if np.issubdtype(x.dtype, np.integer):
        x = x.astype(np.float64) / np.iinfo(x.dtype).max
    return fs, x.astype(np.float64)


def stft_db(x, fs, start, window, hop=0.01, pad=4):
    n = int(window * fs)
    h = int(hop * fs)
    nfft = 1 << int(np.ceil(np.log2(n * pad)))
    # Blackman-Harris (4 terms): sidelobes below -92 dB, so a strong partial's skirt isn't taken for others
    m = np.arange(n) / (n - 1)
    w = 0.35875 - 0.48829 * np.cos(2 * np.pi * m) + 0.14128 * np.cos(4 * np.pi * m) - 0.01168 * np.cos(6 * np.pi * m)
    frames, times = [], []
    for s in range(start, len(x) - n, h):
        spec = np.abs(np.fft.rfft(x[s:s + n] * w, nfft))
        frames.append(20 * np.log10(spec + 1e-12))
        times.append((s + n / 2) / fs)
    return np.array(frames), np.array(times), fs / nfft


def refine(row, k):
    """Parabolic interpolation of a peak at bin k (in dB): (bin offset, level)."""
    if k <= 0 or k >= len(row) - 1:
        return 0.0, row[k]
    a, b, c = row[k - 1], row[k], row[k + 1]
    d = a - 2 * b + c
    off = 0.5 * (a - c) / d if d != 0 else 0.0
    return off, b - 0.25 * (a - c) * off


def analyze(x, fs, max_partials=40, fmin=40.0, fmax=3000.0, window=0.2, start_after=0.03):
    onset = int(np.argmax(np.abs(x)))
    start = onset + int(start_after * fs)
    S, t, df = stft_db(x, fs, start, window)
    if len(S) < 5:
        raise ValueError("the recording is too short after the onset")
    lo, hi = int(fmin / df), min(int(fmax / df), S.shape[1] - 2)
    first = S[:3].mean(axis=0)
    top = first[lo:hi].max()
    # peaks: prominent, within 60 dB of the strongest, a main lobe apart (Blackman-Harris: ±4 bins of 1/window)
    lobe = max(2, int(4.0 / window / df))
    pk, _ = find_peaks(first[lo:hi], height=top - 60, prominence=6, distance=lobe)
    pk = pk + lo
    pk = sorted(pk, key=lambda k: -first[k])[:max_partials]
    # the noise floor of each frame (the median over the band)
    floor = np.median(S[:, lo:hi], axis=1)
    partials = []
    for k0 in sorted(pk):
        k = k0
        freqs, levels = [], []
        for i in range(len(S)):
            span = max(2, int(0.01 * k0))
            a, b = max(lo, k - span), min(hi, k + span + 1)
            k = a + int(np.argmax(S[i, a:b]))
            off, lev = refine(S[i], k)
            if lev < floor[i] + 10 or lev < levels[0] - 60 if levels else lev < floor[i] + 10:
                break
            freqs.append((k + off) * df)
            levels.append(lev)
        if len(levels) < 5:
            continue
        tt = t[:len(levels)]
        slope, icpt = np.polyfit(tt - tt[0], levels, 1)
        t60 = -60.0 / slope if slope < 0 else float("inf")
        # the level at the onset (the line extended back), so fast and slow partials compare
        icpt = icpt + slope * (onset / fs - tt[0])
        partials.append({
            "freq": float(np.median(freqs)),
            "freq_start": float(freqs[0]),
            "glide_cents": float(1200 * np.log2(freqs[0] / freqs[-1])),
            "t60": float(t60),
            "level_db": float(icpt),
            "frames": len(levels),
        })
    if partials:
        ref = max(p["level_db"] for p in partials)
        for p in partials:
            p["amp"] = float(10 ** ((p["level_db"] - ref) / 20))
    return partials


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("wav")
    ap.add_argument("--out")
    ap.add_argument("--channel", type=int, default=0)
    ap.add_argument("--max-partials", type=int, default=40)
    ap.add_argument("--fmin", type=float, default=40.0)
    ap.add_argument("--fmax", type=float, default=3000.0)
    ap.add_argument("--window", type=float, default=0.2)
    ap.add_argument("--start-after", type=float, default=0.03)
    a = ap.parse_args(argv)
    fs, x = read_wav(a.wav, a.channel)
    partials = analyze(x, fs, a.max_partials, a.fmin, a.fmax, a.window, a.start_after)
    out = {"source": str(a.wav), "fs": int(fs), "partials": partials}
    text = json.dumps(out, indent=2)
    if a.out:
        Path(a.out).write_text(text)
        print(f"{len(partials)} partials -> {a.out}", file=sys.stderr)
    else:
        print(text)


if __name__ == "__main__":
    main()
