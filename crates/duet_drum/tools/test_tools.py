"""Tests for analyze.py and compare.py on a signal with known partials.

    python tools/test_tools.py
"""
import json
import sys
import tempfile
from pathlib import Path

import numpy as np
from scipy.io import wavfile

sys.path.insert(0, str(Path(__file__).parent))
import analyze  # noqa: E402
import compare  # noqa: E402

KNOWN = [(110.0, 2.0, 1.0), (173.3, 0.8, 0.5), (241.7, 0.3, 0.4), (302.1, 1.2, 0.3), (517.0, 0.15, 0.2)]


def synth(fs=48000, seconds=4.0, noise=1e-5):
    t = np.arange(int(fs * seconds)) / fs
    x = np.zeros_like(t)
    for f, t60, a in KNOWN:
        x += a * np.exp(-np.log(1000) / t60 * t) * np.sin(2 * np.pi * f * t)
    rng = np.random.default_rng(1)
    return x + noise * rng.standard_normal(len(t))


def test_analyze_finds_the_partials():
    fs = 48000
    parts = analyze.analyze(synth(fs), fs, start_after=0.0)
    for f, t60, a in KNOWN:
        p = min(parts, key=lambda p: abs(p["freq"] - f))
        assert abs(p["freq"] - f) < 0.3, (f, p)
        assert abs(p["t60"] / t60 - 1) < 0.1, (f, t60, p["t60"])
        assert abs(20 * np.log10(p["amp"] / a)) < 1.5, (f, a, p["amp"])  # (within 1.5 dB: the onset is the loudest sample)
    assert len(parts) == len(KNOWN), [p["freq"] for p in parts]
    strongest = max(parts, key=lambda p: p["amp"])
    assert abs(strongest["freq"] - 110) < 0.3 and strongest["amp"] == 1.0
    print("analyze: ok", [(round(p["freq"], 1), round(p["t60"], 3)) for p in parts])


def test_the_command_line_and_compare_round_trip():
    fs = 48000
    with tempfile.TemporaryDirectory() as d:
        wav = Path(d) / "x.wav"
        wavfile.write(wav, fs, synth(fs).astype(np.float32))
        js = Path(d) / "x.json"
        analyze.main([str(wav), "--out", str(js), "--start-after", "0"])
        model = {"source": "truth", "partials": [{"freq": f * 2 ** (10 / 1200), "t60": t, "amp": a, "label": f"p{f}"} for f, t, a in KNOWN]}
        mj = Path(d) / "m.json"
        mj.write_text(json.dumps(model))
        rows = compare.match(json.loads(js.read_text())["partials"], model["partials"])
        assert all(q is not None and abs(c - 10) < 3 for _, q, c in rows), rows
        compare.main([str(js), str(mj), "--wav-a", str(wav), "--wav-b", str(wav), "--out", str(Path(d) / "c.png")])
        assert (Path(d) / "c.png").stat().st_size > 10000
    print("compare: ok (every partial found 10 cents off, the plot drawn)")


if __name__ == "__main__":
    test_analyze_finds_the_partials()
    test_the_command_line_and_compare_round_trip()
    print("all good")
