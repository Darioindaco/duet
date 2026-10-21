"""Self-test without a sound card, against the Rust engine (duet_engine).

The engine's own unit tests (jitter buffer, effects, codes) run with
`cargo test --release` in engine/. This checks the pieces end to end:

    python selftest.py          # add --stun to also check the public address lookup
"""
from __future__ import annotations

import sys
import time

import numpy as np

from duet_engine import Engine

SR, N = 48000, 128


def connect_pair():
    a, b = Engine(), Engine()
    for e in (a, b):
        e.start()
    a.connect(f"127.0.0.1:{b.local_port}")
    b.connect(f"127.0.0.1:{a.local_port}")
    t0 = time.monotonic()
    while not (a.connected and b.connected):
        if time.monotonic() - t0 > 3:
            raise AssertionError("engines never connected")
        time.sleep(0.01)
    return a, b, time.monotonic() - t0


def test_two_engines(seconds: float = 3.0):
    a, b, took = connect_pair()
    print(f"handshake: ok  ({took * 1000:.0f} ms)")
    # Drive both at the real block rate; A sends a click every 100 blocks.
    period = N / SR
    silence = [0.0] * N
    click = [0.9] + [0.0] * (N - 1)
    sent_at, heard_at = [], []
    start = time.perf_counter()
    for k in range(int(seconds / period)):
        x = silence
        if k % 100 == 10:
            x = click
            sent_at.append(k)
        b_out = b.process(silence)
        a.process(x)
        if b_out[0] > 0.5:
            heard_at.append(k)
        delay = start + (k + 1) * period - time.perf_counter()
        if delay > 0:
            time.sleep(delay)
    time.sleep(1.2)  # let a ping round-trip finish
    s = b.state()
    lat = [h - s_ for s_, h in zip(sent_at, heard_at)]
    print(f"clicks sent {len(sent_at)}, heard {len(heard_at)}, delay in blocks {lat}")
    print(f"rtt {a.state()['rtt'] and round(a.state()['rtt'], 2)} ms, B buffer: underruns {s['underruns']} "
          f"lost {s['lost']} late {s['late']} trimmed {s['trimmed']}")
    assert len(heard_at) == len(sent_at), "clicks went missing"
    assert all(0 < d <= 8 for d in lat), "end-to-end delay out of range"
    a.close()
    b.close()
    print("two engines: ok")


def test_signals():
    a, b, _ = connect_pair()
    for e in (a, b):
        e.set_test_signal_loss(0.3)
    big = "x" * 5000 + "é"
    msgs = [big, '{"candidate": 1}', '{"candidate": 2}']
    for m in msgs:
        a.send_signal(m)
    got = []
    t0 = time.monotonic()
    while len(got) < 3 and time.monotonic() - t0 < 8:
        got += b.drain_signals()
        time.sleep(0.05)
    took = time.monotonic() - t0
    time.sleep(1.5)            # retransmits of acked messages must not deliver twice
    got += b.drain_signals()
    assert sorted(got) == sorted(msgs), [len(g) for g in got]
    assert a.signals_pending == 0, "messages never acked"
    a.close()
    b.close()
    print(f"signals: ok  (3 messages incl. 5 KB over a 30%-loss link in {took:.1f} s)")


def test_effects_cpu():
    """Worst realistic load: 4 inputs, each with all six effects, monitored."""
    e = Engine()
    kinds = [fx["name"] for fx in Engine.effects()]
    for i in range(4):
        t = f"L{i}"
        e.strip_set(t, "active", 1)
        e.strip_set(t, "input", i)
        e.strip_set(t, "mon", 1)
        for kind in kinds:
            e.rack_add(t, kind)
    x = (0.3 * np.random.default_rng(0).standard_normal(4 * N)).astype(np.float32).tolist()
    for _ in range(600):
        out = e.process(x, 4)
    assert np.all(np.isfinite(out))
    us = e.state()["cpu_us"]
    print(f"effects: ok  (4 strips x 6 effects: {us:.1f} us per block of {N / SR * 1e6:.0f} us)")
    assert us < 1500, us
    assert set(Engine.presets()) == {"Clean", "Dreamy", "Crunch", "Vocal", "Surf"}
    e.rack_preset("L0", "Crunch")
    assert [s["kind"] for s in e.mixer()["local"][0]["rack"]] == ["comp", "drive", "reverb"]
    e.close()


def run_blocks(a, b, blocks, a_input, b_right=False):
    """Drive A (2 interface inputs) and B in real time; return B's output peak per block."""
    period = N / SR
    silence = [0.0] * N
    peaks = []
    start = time.perf_counter()
    for k in range(blocks):
        out = b.process(silence, 1, b_right)
        a.process(a_input(k), 2)
        peaks.append(max(abs(v) for v in out))
        delay = start + (k + 1) * period - time.perf_counter()
        if delay > 0:
            time.sleep(delay)
    return peaks


def test_mixer():
    a, b, _ = connect_pair()
    a.strip_set("L1", "active", 1)  # second strip: interface input 2, sending by default

    def two_inputs(k):
        x = [0.0] * (2 * N)          # interleaved: input 1, input 2
        if k % 100 == 10:
            x[0] = 0.9               # loud click on input 1
        if k % 100 == 50:
            x[1] = 0.5               # softer click on input 2
        return x

    def clicks(peaks):
        return sorted({round(p, 2) for p in peaks if p > 0.05})

    both = clicks(run_blocks(a, b, 400, two_inputs))
    st_a, st_b = a.state(), b.state()
    assert st_a["sent_channels"] == 2 and st_b["remote_channels"] == 2, (st_a["sent_channels"], st_b["remote_channels"])
    # centre pan is -3 dB per side: 0.9 -> 0.64, 0.5 -> 0.35
    assert both == [0.35, 0.64], both

    b.strip_set("R1", "mute", 1)
    muted = clicks(run_blocks(a, b, 300, two_inputs))
    assert muted == [0.64], muted

    b.strip_set("R0", "pan", 1.0)      # hard right
    left = clicks(run_blocks(a, b, 300, two_inputs))
    right = clicks(run_blocks(a, b, 300, two_inputs, b_right=True))
    assert left == [] and right == [0.9], (left, right)

    # a rack on the partner's channel: reverb leaves a tail after each click
    b.strip_set("R0", "pan", 0.0)
    b.rack_add("R0", "reverb")
    b.rack_set("R0", 0, "mix", 0.8)
    tail = run_blocks(a, b, 300, two_inputs)
    assert sum(p > 0.002 for p in tail) > 3 * len(clicks(tail)) + 30, "no reverb tail"
    m = b.mixer()
    assert m["remote"][0]["rack"][0]["kind"] == "reverb" and m["remote"][1]["mute"]
    a.close()
    b.close()
    print(f"mixer: ok  (2 channels sent and received separately; mute, pan, rack on a partner channel)")


def test_old_build_compat():
    """New engine <-> the original Python engine (from git history): mono both ways."""
    import subprocess
    import tempfile
    from pathlib import Path
    here = Path(__file__).resolve().parent
    old = Path(tempfile.mkdtemp())
    rev = "6fd846c~1"  # last commit with the Python engine
    for name in ("engine.py", "net.py", "effects.py"):
        src = subprocess.run(["git", "show", f"{rev}:{name}"], cwd=here, capture_output=True, check=True).stdout
        (old / name).write_bytes(src)
    sys.path.insert(0, str(old))
    try:
        from engine import Engine as OldEngine  # noqa: E402
    finally:
        sys.path.pop(0)
    new, oldie = Engine(), OldEngine()
    new.start()
    oldie.start()
    new.strip_set("L1", "active", 1)
    new.connect(f"127.0.0.1:{oldie.local_addr[1]}")
    oldie.connect(f"127.0.0.1:{new.local_port}")
    t0 = time.monotonic()
    while not (new.connected and oldie.connected) and time.monotonic() - t0 < 3:
        time.sleep(0.01)
    heard_new, heard_old = 0, 0
    start = time.perf_counter()
    for k in range(400):
        click = k % 100 == 10
        x2 = [0.0] * (2 * N)
        x1 = np.zeros(N, np.float32)
        if click:
            x2[1] = 0.9   # on the new side's second input: old side must still hear it (mono mix)
            x1[0] = 0.9
        heard_new += max(new.process(x2, 2)) > 0.5
        heard_old += float(np.max(oldie.process(x1))) > 0.5
        d = start + (k + 1) * N / SR - time.perf_counter()
        if d > 0:
            time.sleep(d)
    assert new.state()["peer_protocol"] == 1
    new.close()
    oldie.close()
    assert heard_new == 4 and heard_old == 4, (heard_new, heard_old)
    print("old build compat: ok  (new -> old as a mono mix, old -> new as one channel)")


def test_stun():
    e = Engine()
    e.start()  # the receive thread picks up the STUN reply
    code = e.discover()
    print(f"stun: public {e.public_addr}, code {code}")
    assert e.public_addr, "no STUN server answered"
    e.close()


if __name__ == "__main__":
    test_effects_cpu()
    test_two_engines()
    test_signals()
    test_mixer()
    if "--compat" in sys.argv:   # needs git history
        test_old_build_compat()
    if "--stun" in sys.argv:
        test_stun()
    print("all good")
