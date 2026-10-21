"""Self-test without a sound card: two engines talk over localhost.

Checks the jitter buffer on reordered / lost / bursty input, then runs two
real Engines in real time and verifies that an impulse sent by A comes out of
B, and how many blocks it took.

    python selftest.py          # add --stun to also check the public address lookup
"""
from __future__ import annotations

import sys
import time

import numpy as np

import net
from engine import Engine, JitterBuffer


def frame(v, n=128):
    return np.full(n, v, dtype=np.float32)


def test_jitter_buffer():
    jb = JitterBuffer(target=3)
    assert jb.pop() is None
    # out-of-order arrival: 1, 0, 2 -> must play 0, 1, 2
    for s in (1, 0, 2):
        jb.push(s, frame(s))
    assert [jb.pop()[0] for _ in range(3)] == [0, 1, 2]
    # starvation: nothing new -> None, and the late frame 3 still plays afterwards
    assert jb.pop() is None and jb.underruns == 1
    jb.push(3, frame(3))
    assert jb.pop()[0] == 3
    # loss: 4 never arrives but 5 does -> skip 4, play 5
    jb.push(5, frame(5))
    assert jb.pop() is None and jb.lost == 1
    assert jb.pop()[0] == 5
    # driver-style bursts (4 frames in, 4 out) must never trim anything
    s = 6
    for _ in range(300):
        for _ in range(4):
            jb.push(s, frame(s))
            s += 1
        for _ in range(4):
            assert jb.pop() is not None
    assert jb.dropped == 0, jb.dropped
    # sustained excess (partner clock fast): trimmed after the watch window
    for _ in range(5):
        jb.push(s, frame(s))
        s += 1
    for _ in range(450):  # > 2 windows, so at least one window sees only the excess
        jb.push(s, frame(s))
        s += 1
        jb.pop()
    # depth at pop time was 6 (target 3 + 3 extra) -> 3 trimmed, back to 3 at pop / 2 after
    assert jb.dropped == 3 and jb.depth == 2, (jb.dropped, jb.depth)
    # huge pile-up: trimmed right away
    for _ in range(60):
        jb.push(s, frame(s))
        s += 1
    assert jb.pop()[0] == s - 3
    # frame that arrives after its slot is counted late, not played
    jb.push(10, frame(10))
    assert jb.late == 1
    # partner restart: seq counter jumps far away -> buffer resets and refills
    for s in range(3):
        jb.push(90000 + s, frame(100 + s))
    assert jb.pop()[0] == 100
    print("jitter buffer: ok")


def test_effects():
    from effects import PRESETS, Chain
    sr, n = 48000, 128
    rng = np.random.default_rng(0)

    # every effect on its own: finite, bounded, and not silent
    for name in Chain(sr).effects:
        for block in (64, 128, 512):
            ch = Chain(sr)
            ch.set(name, "on", True)
            x = (0.3 * rng.standard_normal(sr * 2)).astype(np.float32)
            y = np.concatenate([ch.process(x[i:i + block]) for i in range(0, len(x), block)])
            assert np.all(np.isfinite(y)), name
            assert np.max(np.abs(y)) < 4, (name, block, np.max(np.abs(y)))
            assert np.std(y) > 0.01, name

    # delay: an impulse comes back after the set time
    ch = Chain(sr)
    ch.set("delay", "on", True)
    ch.set("delay", "time", 0.0)  # 60 ms
    ch.set("delay", "feedback", 0.0)
    x = np.zeros(sr, dtype=np.float32)
    x[0] = 1.0
    y = np.concatenate([ch.process(x[i:i + n]) for i in range(0, sr, n)])
    echo = np.argmax(np.abs(y[100:])) + 100
    assert abs(echo - int(0.06 * sr)) <= 1, echo

    # reverb with max size must decay, not ring forever
    ch = Chain(sr)
    ch.set("reverb", "on", True)
    ch.set("reverb", "size", 1.0)
    ch.set("reverb", "damp", 0.0)
    x = np.zeros(sr * 8, dtype=np.float32)
    x[:n] = 1.0
    y = np.concatenate([ch.process(x[i:i + n]) for i in range(0, len(x), n)])
    early, late = np.max(np.abs(y[:sr])), np.max(np.abs(y[-sr:]))
    assert late < early * 0.1, (early, late)

    # CPU: everything on at once must fit easily in one 128-sample block (2.7 ms)
    ch = Chain(sr)
    for name in ch.effects:
        ch.set(name, "on", True)
    x = (0.3 * rng.standard_normal(n)).astype(np.float32)
    for _ in range(50):
        ch.process(x)
    t0 = time.perf_counter()
    for _ in range(1000):
        ch.process(x)
    per_block = (time.perf_counter() - t0)  # ms per block (1000 blocks)
    for p in PRESETS:
        ch.preset(p)
    print(f"effects: ok  (all six on: {per_block:.3f} ms per block of {n / sr * 1000:.1f} ms)")
    assert per_block < 1.0, per_block


def test_codes():
    code = net.make_code(("203.0.113.7", 50123), ("192.168.1.20", 50123))
    assert net.parse_code(code) == [("203.0.113.7", 50123), ("192.168.1.20", 50123)]
    assert net.parse_code(code.lower()) == net.parse_code(code)
    assert net.parse_code("127.0.0.1:4000") == [("127.0.0.1", 4000)]
    print(f"codes: ok  (example: {code})")


def test_two_engines(seconds: float = 3.0):
    sr, n = 48000, 128
    a, b = Engine(sr, n), Engine(sr, n)
    for e in (a, b):
        e.start()
    a.connect(f"127.0.0.1:{b.local_addr[1]}")
    b.connect(f"127.0.0.1:{a.local_addr[1]}")

    t0 = time.monotonic()
    while not (a.connected and b.connected):
        if time.monotonic() - t0 > 3:
            raise AssertionError("engines never connected")
        time.sleep(0.01)
    print(f"handshake: ok  ({(time.monotonic() - t0) * 1000:.0f} ms)")

    # Drive both at the real block rate. A sends a click every 100 blocks;
    # record when it comes out of B.
    period = n / sr
    blocks = int(seconds / period)
    sent_at, heard_at = [], []
    silence = np.zeros(n, dtype=np.float32)
    start = time.perf_counter()
    for k in range(blocks):
        x = silence
        if k % 100 == 10:
            x = silence.copy()
            x[0] = 0.9
            sent_at.append(k)
        b_out = b.process(silence)
        a.process(x)
        if b_out[0] > 0.5:
            heard_at.append(k)
        # sleep until the next block boundary
        delay = start + (k + 1) * period - time.perf_counter()
        if delay > 0:
            time.sleep(delay)

    time.sleep(1.2)  # let a ping round-trip finish
    lat = [h - s for s, h in zip(sent_at, heard_at)]
    print(f"clicks sent {len(sent_at)}, heard {len(heard_at)}, delay in blocks {lat}")
    print(f"rtt {a.rtt_ms and round(a.rtt_ms, 2)} ms, B buffer: underruns {b.jitter.underruns} "
          f"lost {b.jitter.lost} late {b.jitter.late} dropped {b.jitter.dropped}")
    assert len(heard_at) == len(sent_at), "clicks went missing"
    assert all(0 < d <= 8 for d in lat), "end-to-end delay out of range"
    a.close()
    b.close()
    print("two engines: ok")


def test_signals():
    a, b = Engine(), Engine()
    # lossy link: drop 30% of A's outgoing signal packets and B's acks
    rng = np.random.default_rng(1)
    for e in (a, b):
        real = e._send
        def lossy(data, addr=None, _real=real):
            if data[2] in (7, 6) and rng.random() < 0.3:
                return
            _real(data, addr)
        e._send = lossy
    for e in (a, b):
        e.start()
    a.connect(f"127.0.0.1:{b.local_addr[1]}")
    b.connect(f"127.0.0.1:{a.local_addr[1]}")
    t0 = time.monotonic()
    while not (a.connected and b.connected) and time.monotonic() - t0 < 3:
        time.sleep(0.01)
    big = "x" * 5000 + "é"
    msgs = [big, '{"candidate": 1}', '{"candidate": 2}']
    for m in msgs:
        a.send_signal(m)
    got = []
    t0 = time.monotonic()
    while len(got) < 3 and time.monotonic() - t0 < 8:
        got += b.drain_signals()
        time.sleep(0.05)
    time.sleep(1.5)              # let retransmits of acked messages die out
    got += b.drain_signals()     # ...and make sure none were delivered twice
    assert sorted(got) == sorted(msgs), [len(g) for g in got]
    assert not a._sig_outbox, "messages never acked"
    a.close()
    b.close()
    print(f"signals: ok  (3 messages incl. 5 KB over a 30%-loss link in {time.monotonic() - t0:.1f} s)")


def test_stun():
    e = Engine()
    e.start()   # the receive loop delivers the STUN reply
    code = e.discover()
    print(f"stun: public {e.public_addr}, local {e.local_addr}, code {code}")
    assert e.public_addr, "no STUN server answered"
    e.close()


if __name__ == "__main__":
    test_jitter_buffer()
    test_effects()
    test_codes()
    test_two_engines()
    test_signals()
    if "--stun" in sys.argv:
        test_stun()
    print("all good")
