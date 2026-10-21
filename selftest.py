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
    e = Engine()
    for fx in ("comp", "drive", "chorus", "tremolo", "delay", "reverb"):
        e.set_fx(fx, "on", 1)
    e.set_mix("room", 0.3)
    x = (0.3 * np.random.default_rng(0).standard_normal(N)).astype(np.float32).tolist()
    for _ in range(600):
        out = e.process(x)
    assert np.all(np.isfinite(out))
    us = e.state()["cpu_us"]
    print(f"effects: ok  (all six + room: {us:.1f} us per block of {N / SR * 1e6:.0f} us)")
    assert us < 500, us
    assert set(Engine.presets()) == {"Clean", "Dreamy", "Crunch", "Vocal", "Surf"}
    st = e.preset("Crunch")
    assert st["drive"]["on"] and not st["delay"]["on"]
    e.close()


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
    if "--stun" in sys.argv:
        test_stun()
    print("all good")
