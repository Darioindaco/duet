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


def wait_packets(b, n, timeout=1.0):
    """Lockstep driving: block until B has received A's n-th packet. Pacing with
    sleep() made these tests flaky on slow shared CI machines (sleeps overshoot,
    the jitter buffer fills up); this way timing noise can't change the result."""
    t0 = time.monotonic()
    while b.state()["packets_in"] < n and time.monotonic() - t0 < timeout:
        time.sleep(0.0005)


def test_two_engines(blocks: int = 1100):
    a, b, took = connect_pair()
    print(f"handshake: ok  ({took * 1000:.0f} ms)")
    # A sends a click every 100 blocks; B plays each block once A's packet is in.
    silence = [0.0] * N
    click = [0.9] + [0.0] * (N - 1)
    sent_at, heard_at = [], []
    base = b.state()["packets_in"]
    for k in range(blocks):
        x = silence
        if k % 100 == 10:
            x = click
            sent_at.append(k)
        a.process(x)
        wait_packets(b, base + k + 1)
        if b.process(silence)[0] > 0.5:
            heard_at.append(k)
    time.sleep(1.2)  # let a ping round-trip finish
    s = b.state()
    lat = [h - s_ for s_, h in zip(sent_at, heard_at)]
    print(f"clicks sent {len(sent_at)}, heard {len(heard_at)}, delay in blocks {lat}")
    print(f"rtt {a.state()['rtt'] and round(a.state()['rtt'], 2)} ms, B buffer: underruns {s['underruns']} "
          f"lost {s['lost']} late {s['late']} trimmed {s['trimmed']}")
    assert len(heard_at) == len(sent_at), "clicks went missing"
    # in lockstep the jitter buffer sits exactly at its target (3): played 2 blocks later
    assert all(d == 2 for d in lat), "end-to-end delay should be the jitter target"
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
    kinds = [fx["name"] for fx in Engine.effects() if fx["name"] != "vst"]  # a VST slot needs a plugin
    try:
        e.rack_add("L0", "vst")
        raise AssertionError("adding a VST slot without a plugin should be refused")
    except ValueError:
        pass
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
    """Drive A (2 interface inputs) and B in lockstep; return B's output peak per block."""
    silence = [0.0] * N
    peaks = []
    base = b.state()["packets_in"]
    for k in range(blocks):
        a.process(a_input(k), 2)
        wait_packets(b, base + k + 1)
        out = b.process(silence, 1, b_right)
        peaks.append(max(abs(v) for v in out))
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


def test_partner_fx():
    """Effects on the partner's own strip are part of what they send: B hears A's
    delay without having any effects itself."""
    a, b, _ = connect_pair()
    a.rack_add("L0", "delay")
    a.rack_set("L0", 0, "time", 0.0)       # 60 ms = 2880 samples = 22.5 blocks
    a.rack_set("L0", 0, "feedback", 0.0)
    a.rack_set("L0", 0, "mix", 1.0)
    assert b.mixer()["remote"][0]["rack"] == []
    base = b.state()["packets_in"]
    hits = []
    for k in range(60):
        x = [0.0] * N
        if k == 5:
            x[0] = 0.9
        a.process(x)
        wait_packets(b, base + k + 1)
        out = b.process([0.0] * N)
        for i, v in enumerate(out):
            if abs(v) > 0.1:
                hits.append(k * N + i)
    a.close()
    b.close()
    # the note, then its echo 2880 samples later (the delay smooths it over 2 samples)
    assert hits and any(abs(h - hits[0] - 2880) <= 2 for h in hits), hits[:6]
    print("partner fx: ok  (her delay is in what you hear, with nothing on your side)")


def test_metronome():
    """Leader A plays exactly on its beats; follower B's click must land on A's
    playing as it arrives at B (not on A's clock), every beat."""
    a, b, _ = connect_pair()
    bpm = 150.0                      # 19200 samples per beat = exactly 150 blocks of 128
    epoch = a.metro_lead(True, bpm, 4)
    b.metro_follow(True, bpm, 4, epoch)
    b.metro_volume(0.0)              # B's output = only A's audio; its beats show in state()
    a_clicks, heard, b_beats = [], [], []
    base = b.state()["packets_in"]
    last_beat_b = b.state()["metro_beat"]
    seq0 = a.state()  # noqa: F841  (keeps the shapes symmetric for readability)
    for k in range(700):
        a_seq = epoch - 2 + k        # metro_lead put beat 0 two blocks ahead
        x = [0.0] * N
        if a_seq >= epoch and (a_seq - epoch) % 150 == 0:
            x[0] = 0.9               # A plays on its beat
        a_out = a.process(x)
        loud = max(abs(v) for v in a_out) > 0.05
        if loud and not (a_clicks and a_clicks[-1][1] == k - 1):
            a_clicks.append((k, k))      # a click starts (it rings for ~13 blocks)
        elif loud:
            a_clicks[-1] = (a_clicks[-1][0], k)
        wait_packets(b, base + k + 1)
        out = b.process([0.0] * N)
        if out[0] > 0.3:
            heard.append(k)
        beat = b.state()["metro_beat"]
        if beat != last_beat_b:
            b_beats.append(k)
            last_beat_b = beat
    synced = b.state()["metro_synced"]
    a.close()
    b.close()
    assert synced, "follower never locked to the leader"
    assert len(heard) >= 4, heard
    # each beat B clicks on is the block where A's on-the-beat note arrives
    assert heard == b_beats[-len(heard):], (heard, b_beats)
    onsets = [s for s, _ in a_clicks]
    assert onsets[:4] == [2, 152, 302, 452], onsets[:5]   # the leader hears its own click on time
    print(f"metronome: ok  (follower's click lands on the leader's playing as heard: blocks {heard[:4]}…)")


def standard_nam_file(path):
    """A NAM file with the exact shape of a "standard" capture (2-array WaveNet,
    16/8 channels, 10 dilations each = 13,802 weights). Random weights, so it
    doesn't sound like an amp, but it costs exactly what a real one costs."""
    import json
    dil = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512]
    layers = [
        {"input_size": 1, "condition_size": 1, "channels": 16, "head_size": 8, "kernel_size": 3,
         "dilations": dil, "activation": "Tanh", "gated": False, "head_bias": False},
        {"input_size": 16, "condition_size": 1, "channels": 8, "head_size": 1, "kernel_size": 3,
         "dilations": dil, "activation": "Tanh", "gated": False, "head_bias": True},
    ]
    weights = (np.random.default_rng(7).standard_normal(13802) * 0.05).tolist()
    model = {"version": "0.5.4", "architecture": "WaveNet",
             "metadata": {"name": "Selftest Standard", "loudness": -20.0, "gear_type": "amp_cab"},
             "config": {"layers": layers, "head": None, "head_scale": 0.02}, "weights": weights,
             "sample_rate": 48000}
    with open(path, "w") as f:
        json.dump(model, f)


def test_nam():
    import tempfile
    from pathlib import Path
    path = str(Path(tempfile.gettempdir()) / "duet_selftest_standard.nam")
    standard_nam_file(path)
    e = Engine()
    e.strip_set("L0", "mon", 1)
    e.rack_add("L0", "nam")
    slot = e.mixer()["local"][0]["rack"][0]
    assert slot["kind"] == "nam" and slot["model"] is None
    e.rack_set_model("L0", 0, path)
    slot = e.mixer()["local"][0]["rack"][0]
    assert slot["model_name"] == "Selftest Standard" and slot["model_sr"] == 48000, slot
    assert slot["includes_cab"] is True
    # a broken path raises and leaves the loaded model alone
    try:
        e.rack_set_model("L0", 0, path + ".missing")
        raise AssertionError("missing file should raise")
    except RuntimeError:
        pass
    assert e.mixer()["local"][0]["rack"][0]["model_name"] == "Selftest Standard"
    # it really changes the sound, and the cost of a standard model per block
    x = (0.2 * np.random.default_rng(1).standard_normal(N)).astype(np.float32).tolist()
    dry = Engine()
    dry.strip_set("L0", "mon", 1)
    out_dry, out_nam = dry.process(x), None
    for _ in range(400):
        out_nam = e.process(x)
    assert np.all(np.isfinite(out_nam)) and np.max(np.abs(np.subtract(out_nam, out_dry))) > 1e-3
    us = e.state()["cpu_us"]
    print(f"nam: ok  (standard-size model: {us:.0f} us per block of {N / SR * 1e6:.0f} us)")
    # what matters: one standard model fits in a block with room to spare
    assert us < 0.75 * N / SR * 1e6, us
    # saved and restored with the mixer, model included
    snap = e.mixer()["local"][0]["rack"]
    e2 = Engine()
    e2.rack_load("L0", snap)
    assert e2.mixer()["local"][0]["rack"][0]["model_name"] == "Selftest Standard"
    for x_ in (e, e2, dry):
        x_.close()


TINY_NAM = {
    "version": "0.5.4", "architecture": "WaveNet", "metadata": {"name": "Fake Clean Amp"},
    "config": {"layers": [{"input_size": 1, "condition_size": 1, "channels": 1, "head_size": 1,
                           "kernel_size": 1, "dilations": [1], "activation": "ReLU",
                           "gated": False, "head_bias": False}], "head": None, "head_scale": 1.0},
    "weights": [1.0, 2.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0],
}


def test_tone3000():
    """The whole pick -> token -> models -> download flow against a fake TONE3000
    that checks PKCE and that the token never follows a redirect to storage."""
    import base64
    import hashlib
    import http.server
    import json
    import tempfile
    import threading
    import urllib.parse
    import urllib.request
    from pathlib import Path
    import t3k

    seen = {}

    class Fake(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def reply(self, code, body=b"", ctype="application/json", headers=()):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            for k, v in headers:
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body if isinstance(body, bytes) else json.dumps(body).encode())

        def authed(self):
            return self.headers.get("Authorization") == "Bearer TOK"

        def do_POST(self):
            form = dict(urllib.parse.parse_qsl(self.rfile.read(int(self.headers["Content-Length"])).decode()))
            challenge = base64.urlsafe_b64encode(hashlib.sha256(form["code_verifier"].encode()).digest()).rstrip(b"=").decode()
            ok = (form.get("grant_type") == "authorization_code" and form.get("code") == "CODE123"
                  and challenge == seen["challenge"] and form.get("client_id") == "t3k_pub_test"
                  and form.get("redirect_uri") == seen["redirect"])
            self.reply(200, {"access_token": "TOK", "token_type": "bearer", "expires_in": 3600}) if ok else self.reply(400, {"error": "invalid_grant"})

        def do_GET(self):
            u = urllib.parse.urlsplit(self.path)
            port = self.server.server_port
            if u.path == "/api/v1/tones/42" and self.authed():
                self.reply(200, {"id": 42, "title": "Plexi Crunch", "user": {"username": "tonelover"}, "gear": "amp-cab"})
            elif u.path == "/api/v1/models" and self.authed():
                base = f"http://127.0.0.1:{port}/files"
                self.reply(200, {"data": [
                    {"id": 1, "name": "Cab IR", "size": None, "architecture_version": None, "model_url": f"{base}/ir"},
                    {"id": 2, "name": "Crunch", "size": "lite", "architecture_version": "1", "model_url": f"{base}/2"},
                    {"id": 3, "name": "Clean", "size": "standard", "architecture_version": "2", "model_url": f"{base}/3"},
                ], "page": 1, "total": 3, "total_pages": 1})
            elif u.path.startswith("/files/") and self.authed():
                # like signed storage: a redirect to another host
                self.reply(302, headers=[("Location", f"http://localhost:{port}/storage{u.path[6:]}")])
            elif u.path.startswith("/storage/"):
                if self.headers.get("Authorization"):
                    self.reply(400, {"error": "token leaked to storage"})
                else:
                    self.reply(200, json.dumps(TINY_NAM).encode())
            else:
                self.reply(401, {"error": "unauthorized"})

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fake)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    api_base = f"http://127.0.0.1:{srv.server_port}/api/v1"

    def browser(url):
        q = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(url).query))
        assert q["code_challenge_method"] == "S256" and q["response_type"] == "code"
        assert q["prompt"] == "select_tone" and q["format"] == "nam" and q["client_id"] == "t3k_pub_test"
        seen["challenge"], seen["redirect"] = q["code_challenge"], q["redirect_uri"]
        back = q["redirect_uri"] + "?" + urllib.parse.urlencode({"code": "CODE123", "state": q["state"], "tone_id": "42"})
        threading.Thread(target=lambda: urllib.request.urlopen(back, timeout=5).read(), daemon=True).start()
        return True

    lib = Path(tempfile.mkdtemp())
    assert t3k.Tone3000(lib, lambda: None, api=api_base, open_browser=browser).start_pick()["stage"] == "needs_key"
    t = t3k.Tone3000(lib, lambda: "t3k_pub_test", api=api_base, open_browser=browser)
    t.start_pick()
    t0 = time.monotonic()
    while t.status["stage"] in ("waiting", "loading") and time.monotonic() - t0 < 10:
        time.sleep(0.05)
    st = t.status
    assert st["stage"] == "choose", st
    assert st["tone"]["title"] == "Plexi Crunch" and st["tone"]["author"] == "tonelover"
    assert [m["name"] for m in st["models"]] == ["Clean", "Crunch"], st["models"]  # IR dropped, standard first
    got = t.download(3)
    assert Path(got["path"]).exists() and json.loads(Path(got["path"]).read_text())["metadata"]["name"] == "Fake Clean Amp"
    assert [m["title"] for m in t.library()] == ["Plexi Crunch"]
    e = Engine()
    e.rack_add("L0", "nam")
    e.rack_set_model("L0", 0, got["path"])
    assert e.mixer()["local"][0]["rack"][0]["model_name"] == "Fake Clean Amp"
    e.close()
    srv.shutdown()
    print("tone3000: ok  (PKCE pick, token, models, download without leaking the token, library, loads)")


def test_plugin_path():
    """The vst3 crate's gain example (engine/examples/test_gain.rs), packaged as
    a .vst3. Built with: cd engine && cargo build --release --example test_gain"""
    import shutil
    import tempfile
    from pathlib import Path
    ex = Path(__file__).resolve().parent / "engine" / "target" / "release" / "examples"
    lib = next((p for p in (ex / "test_gain.dll", ex / "libtest_gain.dylib", ex / "libtest_gain.so") if p.exists()), None)
    if lib is None:
        return None
    out = Path(tempfile.mkdtemp()) / "test_gain.vst3"
    if lib.suffix == ".dll":
        shutil.copy(lib, out)                       # a single-file .vst3 is fine on Windows
    else:
        (out / "Contents" / "MacOS").mkdir(parents=True)
        shutil.copy(lib, out / "Contents" / "MacOS" / "test_gain")
        (out / "Contents" / "Info.plist").write_text(
            '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>'
            "<key>CFBundleExecutable</key><string>test_gain</string>"
            "<key>CFBundleIdentifier</key><string>app.duet.testgain</string>"
            "<key>CFBundlePackageType</key><string>BNDL</string></dict></plist>")
    return str(out)


def test_vst():
    path = test_plugin_path()
    if not path:
        print("vst: skipped (build the test plugin: cd engine && cargo build --release --example test_gain)")
        return
    classes = Engine.vst_scan(path)
    assert [c["name"] for c in classes] == ["Gain (vst3-rs example plugin)"], classes
    e = Engine()
    e.strip_set("L0", "mon", 1)
    e.rack_add_vst("L0", path, classes[0]["cid"], classes[0]["name"])
    slot = e.mixer()["local"][0]["rack"][0]
    assert slot["kind"] == "vst" and slot["vst"]["name"].startswith("Gain"), slot
    # stereo-only plugin on a mono strip: the host must fall back to stereo and
    # fold back; at its default gain (1.0) out == in
    dry = Engine()
    dry.strip_set("L0", "mon", 1)
    x = (0.3 * np.random.default_rng(3).standard_normal(N)).astype(np.float32).tolist()
    for _ in range(5):
        wet_out, dry_out = e.process(x), dry.process(x)
    assert np.allclose(wet_out, dry_out, atol=1e-6), np.max(np.abs(np.subtract(wet_out, dry_out)))
    # state survives save + restore into a fresh engine
    snap = e.mixer(True)["local"][0]["rack"]
    assert snap[0]["vst"]["state"], "no plugin state saved"
    e2 = Engine()
    e2.strip_set("L0", "mon", 1)
    e2.rack_load("L0", snap)
    assert np.allclose(e2.process(x), dry_out, atol=1e-6)
    # removing it tears the instance down (off the audio thread)
    e.rack_remove("L0", 0)
    e.process(x)
    assert e.mixer()["local"][0]["rack"] == []
    for z in (e, e2, dry):
        z.close()
    print("vst: ok  (scan, load with stereo fallback, audio, state save/restore, teardown)")


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
    test_partner_fx()
    test_metronome()
    test_nam()
    test_tone3000()
    test_vst()
    if "--compat" in sys.argv:   # needs git history
        test_old_build_compat()
    if "--stun" in sys.argv:
        test_stun()
    print("all good")
