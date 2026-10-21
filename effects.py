"""Pedalboard: real-time effects that run inside the audio callback.

Every effect works on a whole block with numpy, never sample-by-sample Python
(at 128 samples / 48 kHz the callback has 2.7 ms). The trick that makes the
feedback effects (delay, reverb) vectorizable: their delay lines are longer than
the chunk being processed, so every sample they feed back on is already in the
past. Blocks are cut into chunks of at most CHUNK samples to keep that true.

Parameters live in plain dicts that the UI thread writes and the audio thread
reads; single float assignments are atomic in Python, so no locking is needed.
"""
from __future__ import annotations

import numpy as np

CHUNK = 128


class Ring:
    """Circular buffer with vectorized writes and integer / fractional reads."""

    def __init__(self, size: int):
        self.buf = np.zeros(size, dtype=np.float32)
        self.size = size
        self.pos = 0  # index of the next write

    def read(self, delay: int, n: int) -> np.ndarray:
        """The n samples that were written `delay` samples before the next n."""
        idx = (self.pos - delay + np.arange(n)) % self.size
        return self.buf[idx]

    def read_frac(self, delays: np.ndarray) -> np.ndarray:
        """Per-sample fractional delays (for modulation), linear interpolation."""
        n = len(delays)
        pos = self.pos + np.arange(n) - delays
        i0 = np.floor(pos).astype(np.int64)
        frac = (pos - i0).astype(np.float32)
        a = self.buf[i0 % self.size]
        b = self.buf[(i0 + 1) % self.size]
        return a + (b - a) * frac

    def write(self, x: np.ndarray):
        idx = (self.pos + np.arange(len(x))) % self.size
        self.buf[idx] = x
        self.pos = (self.pos + len(x)) % self.size

    def clear(self):
        self.buf[:] = 0


class Effect:
    name = ""
    defaults: dict[str, float] = {}

    def __init__(self, sr: int):
        self.sr = sr
        self.on = False
        self.p = dict(self.defaults)

    def process(self, x: np.ndarray) -> np.ndarray:
        raise NotImplementedError

    def reset(self):
        pass


class Compressor(Effect):
    """Evens out levels. Block-rate envelope, gain ramped across the block."""
    name = "comp"
    defaults = {"threshold": 0.5, "ratio": 0.5, "makeup": 0.3}

    def __init__(self, sr):
        super().__init__(sr)
        self.env = 0.0
        self.gain = 1.0

    def process(self, x):
        rms = float(np.sqrt(np.mean(x * x)) + 1e-9)
        # fast attack, slow release (coefficients per block of ~128)
        coef = 0.5 if rms > self.env else 0.03
        self.env += (rms - self.env) * coef
        thr_db = -40 + 36 * self.p["threshold"]            # -40 .. -4 dBFS
        ratio = 1 + 11 * self.p["ratio"]                   # 1:1 .. 12:1
        env_db = 20 * np.log10(self.env + 1e-9)
        over = max(0.0, env_db - thr_db)
        gr_db = over - over / ratio
        target = 10 ** ((-gr_db + 18 * self.p["makeup"]) / 20)
        ramp = np.linspace(self.gain, target, len(x), dtype=np.float32)
        self.gain = target
        return x * ramp


class Drive(Effect):
    """Overdrive: soft clipping with a tone control."""
    name = "drive"
    defaults = {"drive": 0.4, "tone": 0.5, "level": 0.5}

    def __init__(self, sr):
        super().__init__(sr)
        self.tail = np.zeros(15, dtype=np.float32)

    def process(self, x):
        g = 1 + 40 * self.p["drive"] ** 2
        y = np.tanh(x * g) / np.tanh(g) ** 0.5
        # tone: blend between a darkened copy (moving average) and the raw clip
        taps = 16
        ext = np.concatenate([self.tail, y])
        self.tail = ext[-(taps - 1):]
        dark = np.convolve(ext, np.full(taps, 1 / taps, dtype=np.float32), mode="valid")
        t = self.p["tone"]
        y = dark * (1 - t) + y * t
        return (y * (0.2 + 1.3 * self.p["level"])).astype(np.float32)


class Chorus(Effect):
    """Two modulated short delays mixed with the dry signal."""
    name = "chorus"
    defaults = {"rate": 0.3, "depth": 0.5, "mix": 0.5}

    def __init__(self, sr):
        super().__init__(sr)
        self.ring = Ring(int(sr * 0.05))
        self.phase = 0.0

    def process(self, x):
        n = len(x)
        self.ring.write(x)
        rate = 0.1 + 3.0 * self.p["rate"]                  # Hz
        t = self.phase + 2 * np.pi * rate * np.arange(n) / self.sr
        self.phase = float(t[-1] + 2 * np.pi * rate / self.sr) % (2 * np.pi)
        base = 0.012 * self.sr
        depth = 0.006 * self.sr * self.p["depth"]
        # delays are measured from the end of the block we just wrote
        d1 = base + depth * np.sin(t) + n
        d2 = base + depth * np.sin(t + np.pi / 2) + n
        wet = 0.5 * (self.ring.read_frac(d1) + self.ring.read_frac(d2))
        m = self.p["mix"]
        return (x * (1 - 0.5 * m) + wet * m).astype(np.float32)


class Tremolo(Effect):
    name = "tremolo"
    defaults = {"rate": 0.4, "depth": 0.6}

    def __init__(self, sr):
        super().__init__(sr)
        self.phase = 0.0

    def process(self, x):
        rate = 0.5 + 11.5 * self.p["rate"]
        t = self.phase + 2 * np.pi * rate * np.arange(len(x)) / self.sr
        self.phase = float(t[-1] + 2 * np.pi * rate / self.sr) % (2 * np.pi)
        lfo = 1 - self.p["depth"] * (0.5 + 0.5 * np.sin(t))
        return (x * lfo).astype(np.float32)


class Delay(Effect):
    """Echo with feedback; the repeats get darker like a tape/analog delay."""
    name = "delay"
    defaults = {"time": 0.35, "feedback": 0.4, "mix": 0.35}

    def __init__(self, sr):
        super().__init__(sr)
        self.ring = Ring(int(sr * 1.6))
        self.prev = 0.0

    def process(self, x):
        n = len(x)
        d = int(max(CHUNK, (0.06 + 1.1 * self.p["time"]) * self.sr))
        echoed = self.ring.read(d, n)
        # 2-tap average = gentle lowpass on each repeat (vectorized: both taps are past)
        shifted = np.concatenate([[self.prev], echoed[:-1]]).astype(np.float32)
        self.prev = float(echoed[-1])
        dark = 0.5 * (echoed + shifted)
        fb = 0.92 * self.p["feedback"]
        self.ring.write(x + dark * fb)
        return (x + dark * self.p["mix"]).astype(np.float32)

    def reset(self):
        self.ring.clear()


class Reverb(Effect):
    """Freeverb-style: 8 parallel damped combs into 4 series allpasses."""
    name = "reverb"
    defaults = {"size": 0.6, "damp": 0.4, "mix": 0.3}
    COMBS = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617]
    ALLPASS = [556, 441, 341, 225]

    def __init__(self, sr):
        super().__init__(sr)
        scale = sr / 44100
        self.combs = [(Ring(int(c * scale) + CHUNK + 2), int(c * scale)) for c in self.COMBS]
        self.comb_prev = [0.0] * len(self.combs)
        self.aps = [(Ring(int(a * scale) + CHUNK + 2), int(a * scale)) for a in self.ALLPASS]

    def process(self, x):
        n = len(x)
        fb = 0.7 + 0.28 * self.p["size"]
        damp = self.p["damp"]
        inp = x * 0.015
        out = np.zeros(n, dtype=np.float32)
        for i, (ring, d) in enumerate(self.combs):
            y = ring.read(d, n)
            prev = np.concatenate([[self.comb_prev[i]], y[:-1]]).astype(np.float32)
            self.comb_prev[i] = float(y[-1])
            filtered = y * (1 - damp) + prev * damp
            ring.write(inp + filtered * fb)
            out += y
        for ring, d in self.aps:
            buf = ring.read(d, n)
            ring.write(out + buf * 0.5)
            out = buf - out
        m = self.p["mix"]
        return (x * (1 - 0.5 * m) + out * 3.0 * m).astype(np.float32)

    def reset(self):
        for ring, _ in self.combs + self.aps:
            ring.clear()


ORDER = [Compressor, Drive, Chorus, Tremolo, Delay, Reverb]

PRESETS = {
    "Clean":  {},
    "Dreamy": {"chorus": {"rate": 0.2, "depth": 0.7, "mix": 0.5},
               "delay": {"time": 0.4, "feedback": 0.45, "mix": 0.3},
               "reverb": {"size": 0.85, "damp": 0.3, "mix": 0.45}},
    "Crunch": {"comp": {"threshold": 0.4, "ratio": 0.4, "makeup": 0.2},
               "drive": {"drive": 0.6, "tone": 0.55, "level": 0.45},
               "reverb": {"size": 0.4, "damp": 0.5, "mix": 0.15}},
    "Vocal":  {"comp": {"threshold": 0.35, "ratio": 0.5, "makeup": 0.35},
               "reverb": {"size": 0.55, "damp": 0.45, "mix": 0.25}},
    "Surf":   {"tremolo": {"rate": 0.45, "depth": 0.5},
               "reverb": {"size": 0.75, "damp": 0.2, "mix": 0.5}},
}


class Chain:
    """Effects in a fixed order; only the switched-on ones cost CPU."""

    def __init__(self, sr: int, kinds=ORDER):
        self.sr = sr
        self.effects = {cls.name: cls(sr) for cls in kinds}

    def process(self, x: np.ndarray) -> np.ndarray:
        active = [e for e in self.effects.values() if e.on]
        if not active:
            return x
        if len(x) <= CHUNK:
            for e in active:
                x = e.process(x)
            return x
        out = np.empty_like(x)
        for s in range(0, len(x), CHUNK):
            seg = x[s:s + CHUNK]
            for e in active:
                seg = e.process(seg)
            out[s:s + len(seg)] = seg
        return out

    def set(self, effect: str, key: str, value):
        e = self.effects[effect]
        if key == "on":
            if value and not e.on:
                e.reset()  # don't replay a stale tail from last time it was on
            e.on = bool(value)
        else:
            e.p[key] = float(value)

    def preset(self, name: str):
        cfg = PRESETS[name]
        for e in self.effects.values():
            e.p = dict(type(e).defaults)
            e.p.update(cfg.get(e.name, {}))
            self.set(e.name, "on", e.name in cfg)

    def state(self):
        return {name: {"on": e.on, **e.p} for name, e in self.effects.items()}
