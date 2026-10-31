"""Analysis-resynthesis of one drum hit into the compact recipe a neural model would predict.

    python tools/resynth.py hit.wav [--out resynth.wav] [--json recipe.json] [--partials 60]

The recipe: K partials, each a damped sinusoid with a glide (f(t) = f_end·(1 + g·e^{−t/τ_g}), level
a·e^{−t/τ}); and the rest (the stick's tick, the noise of the attack) as noise in 40 bands, each with a peak
level and a decay. A few hundred numbers per hit. Rendering the recipe and listening against the
original answers: can a recipe like this sound real? Needs numpy, scipy.
"""
from __future__ import annotations

import argparse
import json
import math

import numpy as np
from scipy.io import wavfile
from scipy.signal import find_peaks, stft, istft


def read(path):
    fs, x = wavfile.read(path)
    if x.ndim > 1:
        x = x.mean(axis=1)
    if np.issubdtype(x.dtype, np.integer):
        x = x.astype(np.float64) / np.iinfo(x.dtype).max
    return fs, x.astype(np.float64)


def bh(n):
    m = np.arange(n) / (n - 1)
    return 0.35875 - 0.48829 * np.cos(2 * np.pi * m) + 0.14128 * np.cos(4 * np.pi * m) - 0.01168 * np.cos(6 * np.pi * m)


def partials(x, fs, k_max=60, fmin=30.0, fmax=9000.0):
    """Track the partials frame by frame and fit each with a glide and an exponential decay."""
    n, hop, nfft = 4096, 128, 16384
    w = bh(n)
    pad = np.concatenate([np.zeros(n // 2), x, np.zeros(n)])
    frames = range(0, len(x), hop)
    spec = np.array([np.abs(np.fft.rfft(pad[s:s + n] * w, nfft)) for s in frames])  # frame i is centred at i·hop
    t = np.arange(len(spec)) * hop / fs
    db = 20 * np.log10(spec + 1e-12)
    df = fs / nfft
    lo, hi = int(fmin / df), int(fmax / df)
    # pick in the frames 30-120 ms after the onset (past the window's smear of the attack)
    i0, i1 = int(0.03 * fs / hop), int(0.12 * fs / hop)
    ref = db[i0:i1].max(axis=0)
    top = ref[lo:hi].max()
    pk, _ = find_peaks(ref[lo:hi], height=top - 75, prominence=6, distance=int(16 / df))
    pk = sorted(pk + lo, key=lambda k: -ref[k])[:k_max]
    amp_scale = 2.0 / w.sum()
    out = []
    for k0 in pk:
        f_tr, a_tr, t_tr = [], [], []
        k = k0
        for i in range(int(0.012 * fs / hop), len(spec)):
            span = max(3, int(0.006 * k0))
            a, b = max(lo, k - span), min(hi, k + span + 1)
            k = a + int(np.argmax(db[i, a:b]))
            if db[i, k] < top - 95 or (a_tr and db[i, k] < max(a_tr) - 60):
                break
            al, be, ga = db[i, k - 1], db[i, k], db[i, k + 1]
            d = al - 2 * be + ga
            off = 0.5 * (al - ga) / d if d != 0 else 0.0
            f_tr.append((k + off) * df)
            a_tr.append(be - 0.25 * (al - ga) * off)
            t_tr.append(t[i])
        if len(t_tr) < 8:
            continue
        f_tr, a_tr, t_tr = np.array(f_tr), np.array(a_tr), np.array(t_tr)
        # level: a line in dB from 20 ms on, extended back to the onset (no more than 6 dB above the first frame)
        m = t_tr >= 0.02
        slope, icpt = np.polyfit(t_tr[m], a_tr[m], 1)
        icpt = min(icpt, a_tr[0] + 6)
        tau = -20 / (slope * math.log(10)) if slope < 0 else 10.0
        # glide: f(t) = f_end (1 + g e^{-t/τg}), the best τg of a grid (f_end and g by least squares)
        best = None
        for tg in (0.02, 0.04, 0.07, 0.1, 0.15, 0.25, 0.4):
            e = np.exp(-t_tr / tg)
            A = np.vstack([np.ones_like(e), e]).T
            (c0, c1), res, *_ = np.linalg.lstsq(A, f_tr, rcond=None)
            err = np.sum((A @ np.array([c0, c1]) - f_tr) ** 2)
            if best is None or err < best[0]:
                best = (err, c0, c1 / c0, tg)
        _, f_end, g, tg = best
        out.append({"freq": float(f_end), "glide": float(g), "glide_tau": tg, "amp": float(10 ** (icpt / 20) * amp_scale), "tau": float(tau)})
    return out


def basis(p, t, fs):
    """A partial's cos and sin parts (unit amplitude): its glide and its decay, from the onset."""
    f = p["freq"] * (1 + p["glide"] * np.exp(-t / p["glide_tau"]))
    ph = 2 * np.pi * np.cumsum(f) / fs
    env = np.exp(-t / p["tau"]) * np.clip(t / 0.0005, 0, 1)
    return env * np.cos(ph), env * np.sin(ph)


def fit_amplitudes(ps, x, fs, seconds=0.6):
    """Each partial's amplitude and phase, by least squares on the waveform itself (frequencies, glides and
    decays fixed): the attack's shape comes out right, which the STFT's levels can't give."""
    n = min(len(x), int(seconds * fs))
    t = np.arange(n) / fs
    cols = []
    for p in ps:
        c, s = basis(p, t, fs)
        cols += [c, s]
    A = np.array(cols).T
    coef, *_ = np.linalg.lstsq(A, x[:n], rcond=None)
    for i, p in enumerate(ps):
        a, b = coef[2 * i], coef[2 * i + 1]
        p["amp"] = float(math.hypot(a, b))
        p["phase"] = float(math.atan2(-b, a))  # a·cos + b·sin = amp·cos(ph + phase)
    return ps


def refine(ps, x, fs, top=10, seconds=0.8):
    """The strongest partials' frequency, glide and decay, by nonlinear least squares on the waveform (the
    others held; amplitude and phase solved inside: variable projection). The STFT's estimates of the first
    tens of milliseconds, where the glide is, are too coarse for the partials that carry the tone."""
    from scipy.optimize import least_squares
    n = min(len(x), int(seconds * fs))
    t = np.arange(n) / fs
    order = sorted(range(len(ps)), key=lambda i: -ps[i]["amp"])[:top]
    for i in order:
        others = render_partials([p for j, p in enumerate(ps) if j != i], fs, n)
        target = x[:n] - others
        p0 = ps[i]

        def resid(v):
            q = dict(p0, freq=v[0], glide=v[1], glide_tau=abs(v[2]) + 1e-3, tau=abs(v[3]) + 1e-3)
            c, s_ = basis(q, t, fs)
            A = np.vstack([c, s_]).T
            coef, *_ = np.linalg.lstsq(A, target, rcond=None)
            return A @ coef - target

        v0 = [p0["freq"], p0["glide"], p0["glide_tau"], p0["tau"]]
        r = least_squares(resid, v0, x_scale=[1.0, 0.01, 0.05, 0.3], max_nfev=60)
        q = dict(p0, freq=float(r.x[0]), glide=float(r.x[1]), glide_tau=float(abs(r.x[2]) + 1e-3), tau=float(abs(r.x[3]) + 1e-3))
        if np.sum(resid(r.x) ** 2) < np.sum(resid(v0) ** 2):
            ps[i] = q
    return ps


def render_partials(ps, fs, n):
    t = np.arange(n) / fs
    y = np.zeros(n)
    for p in ps:
        c, s = basis(p, t, fs)
        y += p["amp"] * (math.cos(p.get("phase", 0.0)) * c - math.sin(p.get("phase", 0.0)) * s)
    return y


BANDS = np.geomspace(40, 20000, 41)


def noise_recipe(r, fs, window=0.1, fmin=400.0, min_decay=100.0):
    """The residual's attack as noise in 40 bands: per band a peak level, the time of the peak, and a decay.
    Only the first `window` seconds and only above `fmin`: what's left later and lower is the partials'
    misfit, not noise, and long noise sounds like hiss."""
    r = r[: int(window * fs)]
    f, t, Z = stft(r, fs, nperseg=512, noverlap=384)
    P = np.abs(Z) ** 2
    bands = []
    for a, b in zip(BANDS[:-1], BANDS[1:]):
        m = (f >= a) & (f < b)
        if not m.any() or b <= fmin:
            bands.append(None)
            continue
        e = P[m].mean(axis=0) + 1e-20
        edb = 10 * np.log10(e)
        ip = int(np.argmax(edb))
        tail = edb[ip:]
        ok = tail > tail[0] - 50
        n_ok = max(3, int(np.argmin(ok)) if not ok.all() else len(tail))
        tt = t[ip:ip + n_ok] - t[ip]
        slope = np.polyfit(tt, tail[:n_ok], 1)[0] if n_ok >= 3 else -200.0
        bands.append({"lo": float(a), "hi": float(b), "peak_db": float(edb[ip]), "t_peak": float(t[ip]), "decay_db_s": float(min(slope, -min_decay))})
    return bands


def render_noise(bands, fs, n, seed=0):
    rng = np.random.default_rng(seed)
    f, t, Z = stft(rng.standard_normal(n), fs, nperseg=512, noverlap=384)
    unit = np.mean(np.abs(Z) ** 2)  # white noise's power per bin
    G = np.zeros(Z.shape)
    for bd in bands:
        if bd is None:
            continue
        m = (f >= bd["lo"]) & (f < bd["hi"])
        env = np.where(t < bd["t_peak"], bd["peak_db"] - 120 * (bd["t_peak"] - t) / max(bd["t_peak"], 1e-3), bd["peak_db"] + bd["decay_db_s"] * (t - bd["t_peak"]))
        G[m] = np.sqrt(10 ** (env / 10) / unit)
    _, y = istft(Z * G, fs, nperseg=512, noverlap=384)
    return y[:n]


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("wav")
    ap.add_argument("--out", default="resynth.wav")
    ap.add_argument("--json")
    ap.add_argument("--partials", type=int, default=60)
    a = ap.parse_args(argv)
    fs, x = read(a.wav)
    on = int(np.argmax(np.abs(x) > 0.1 * np.abs(x).max()))
    x = x[max(0, on - int(0.001 * fs)):]
    ps = fit_amplitudes(partials(x, fs, a.partials), x, fs)
    ps = fit_amplitudes(refine(ps, x, fs), x, fs)
    sines = render_partials(ps, fs, len(x))
    bands = noise_recipe(x - sines, fs)
    noise = render_noise(bands, fs, len(x))
    y = sines + noise
    wavfile.write(a.out, fs, y.astype(np.float32))
    wavfile.write(a.out.replace(".wav", "_sines.wav"), fs, sines.astype(np.float32))
    n_par = len(ps) * 6 + sum(4 for b in bands if b)
    print(f"{len(ps)} partials + {sum(1 for b in bands if b)} noise bands = {n_par} numbers -> {a.out}")
    if a.json:
        with open(a.json, "w") as fh:
            json.dump({"fs": fs, "partials": ps, "noise": bands}, fh, indent=1)


if __name__ == "__main__":
    main()
