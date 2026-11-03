"""Self-test without a sound card, against the Rust engine (duet_engine).

The engine's own unit tests (jitter buffer, effects, codes) run with
`cargo test --release` in engine/. This checks the pieces end to end:

    python selftest.py          # add --stun to also check the public address lookup
"""
from __future__ import annotations

import math
import re
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


def test_auto_buffer(blocks: int = 375 * 23):
    """Auto safety buffer on a clean link: steps down from 3 to 1 (one block every
    10 s of calm), without a dropout and without the partner's audio going missing."""
    a, b, _ = connect_pair()
    b.set_buffer(3)
    b.set_buffer_auto(True)
    silence = [0.0] * N
    base = b.state()["packets_in"]
    targets = []
    for k in range(blocks):
        a.process(silence)
        wait_packets(b, base + k + 1)
        b.process(silence)
        if k % 375 == 0:
            targets.append(b.state()["jitter_target"])
    s = b.state()
    print(f"auto buffer: target every 5 s {targets[::5]}, underruns {s['underruns']} trimmed {s['trimmed']}")
    assert s["jitter_auto"] and s["jitter_target"] == 1, "auto should settle at 1 block on a clean link"
    assert s["underruns"] == 0 and s["trimmed"] == 2
    b.set_buffer(2)  # a manual value switches auto off
    a.process(silence)
    b.process(silence)
    assert not b.state()["jitter_auto"] and b.state()["jitter_target"] == 2
    a.close()
    b.close()
    print("auto buffer: ok")


def wait_for(cond, timeout=8.0, what="condition", detail=None):
    t0 = time.monotonic()
    while not cond():
        if time.monotonic() - t0 > timeout:
            raise AssertionError(f"timed out waiting for {what}{(': ' + str(detail())) if detail else ''}")
        time.sleep(0.02)


def test_daw():
    """Two people, one song: they end up in the same project, both record a take
    at the same time (each on their own machine), and afterwards both sides
    have both clips, both audio files and an identical project."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw

    a, b, _ = connect_pair()
    root = Path(tempfile.mkdtemp(prefix="duet-daw-"))
    da, db = Daw(a, root / "A", "alice"), Daw(b, root / "B", "bob")
    try:
        # two fresh, empty songs: one side adopts the other's
        wait_for(lambda: da.p.pid == db.p.pid and da.view()["shared"] and db.view()["shared"], what="one shared song",
                 detail=lambda: (da.p.pid, db.p.pid, da.partner_pid, db.partner_pid, da.connected, db.connected, da.conflict, db.conflict))
        # a track for bob's first input, armed, next to alice's own (armed too)
        mine = da.view()["tracks"][0]["id"]
        da.edit([["track", mine, {"arm": True, "owner": "alice"}], ["track", "tbob", {"name": "Bob", "order": 2.0, "owner": "bob", "src": 0,
                                                                   "arm": True, "vol": 0.8, "pan": 0.0, "rack": []}]])
        wait_for(lambda: len(db.view()["tracks"]) == 2 and all(t["arm"] for t in db.view()["tracks"]), what="tracks on bob's side")
        for d in (da, db):
            d.set_local("countin", False)
        assert da.record(0.0) == {"ok": True}
        wait_for(lambda: b.state()["daw_recording"], what="bob recording too")
        sa = [0.25 * math.sin(2 * math.pi * 330 * i / SR) for i in range(N)]
        sb = [0.25 * math.sin(2 * math.pi * 550 * i / SR) for i in range(N)]
        for _ in range(300):
            a.process(sa)
            b.process(sb)
        da.stop()
        wait_for(lambda: not b.state()["daw_playing"], what="bob stopping")
        both = lambda d: len(d.view()["clips"]) == 2 and all(f["here"] for f in d.view()["files"].values()) and len(d.view()["files"]) == 2
        info = lambda d: ([(c["track"], c["file"], round(c["start"], 3)) for c in d.view()["clips"]], {n: f["here"] for n, f in d.view()["files"].items()}, dict(d.wanted))
        wait_for(lambda: both(da) and both(db), timeout=15, what="both clips and files on both sides",
                 detail=lambda: (info(da), info(db), a.file_status(), b.file_status()))
        wait_for(lambda: da.p.digest() == db.p.digest(), what="identical projects")
        va = da.view()
        owners = sorted(va["files"][c["file"]]["owner"] for c in va["clips"])
        assert owners == ["alice", "bob"], owners
        for c in va["clips"]:
            assert abs(c["start"]) < 0.2 and c["len"] > 0.5, c  # started at the top, ~0.8 s long
        for name in va["files"]:
            fa, fb = (root / x / da.p.pid / "audio" / name for x in ("A", "B"))
            assert fa.read_bytes() == fb.read_bytes(), f"{name} differs between the two sides"
        # an edit on one side shows up on the other
        cid = va["clips"][0]["id"]
        db.edit([["clip", cid, {"start": 4.0}]])
        wait_for(lambda: next(c for c in da.view()["clips"] if c["id"] == cid)["start"] == 4.0, what="moved clip on alice's side")
        print(f"daw: ok  (shared song, 2 takes recorded at once, audio swapped: {', '.join(sorted(va['files']))})")
    finally:
        da.close()
        db.close()
        a.close()
        b.close()
        shutil.rmtree(root, ignore_errors=True)


def test_song_edit():
    """Undo/redo of your own edits (a whole take in one step), and the export:
    mix and stems as WAV, the right length, faster than real time."""
    import shutil
    import tempfile
    import wave
    from pathlib import Path

    from daw import Daw

    e = Engine()
    e.start()
    root = Path(tempfile.mkdtemp(prefix="duet-edit-"))
    d = Daw(e, root / "songs", "me")
    try:
        tid = d.view()["tracks"][0]["id"]
        d.edit([["track", tid, {"arm": True}]])
        d.set_local("countin", False)
        d.record(0.0)
        x = [0.3 * math.sin(2 * math.pi * 220 * i / SR) for i in range(N)]
        for _ in range(375):  # 1 s
            e.process(x)
        d.stop()
        clip = d.view()["clips"][0]
        # split it in two, then undo and redo that
        half = clip["len"] / 2
        d.edit([["clip", clip["id"], {"len": half}],
                ["clip", "c-right", {**{k: clip[k] for k in ("track", "file", "gain")}, "start": clip["start"] + half / 0.5,
                                     "offset": half, "len": half, "fade_in": 0.0, "fade_out": 0.0}]])
        assert len(d.view()["clips"]) == 2
        d.undo()
        assert [round(c["len"], 3) for c in d.view()["clips"]] == [round(clip["len"], 3)], "split undone"
        d.redo()
        assert len(d.view()["clips"]) == 2
        d.undo()
        d.undo()
        assert d.view()["clips"] == [], "the take itself undone"
        d.redo()
        assert len(d.view()["clips"]) == 1
        out = root / "out"
        out.mkdir()
        res = d.export(str(out / "mix.wav"), str(out), loop=False)
        assert res.get("ok"), res
        t0 = time.monotonic()
        while (st := e.daw_export_status())["result"] is None:
            assert time.monotonic() - t0 < 20, "export never finished"
            time.sleep(0.02)
        assert st["result"]["ok"], st
        for f in res["files"]:
            with wave.open(f) as w:
                secs = w.getnframes() / w.getframerate()
                assert w.getnchannels() == 2 and w.getsampwidth() == 3 and abs(secs - (clip["len"] + 2.0)) < 0.05, (f, secs)
        loud = st["result"]["loudness"]
        assert loud and loud["integrated"] is not None and loud["true_peak"] is not None and not loud["gain"], loud
        peak0 = wave_peak(out / "mix.wav")
        # normalized to -16 LUFS: the file comes out that loud (turned by the gain it says)
        res = d.export(str(out / "norm.wav"), None, loop=False, normalize=-16.0)
        while (st2 := e.daw_export_status())["result"] is None:
            time.sleep(0.02)
        l2 = st2["result"]["loudness"]
        assert abs(l2["integrated"] - (-16.0)) < 0.15 and abs(l2["gain"] - (-16.0 - loud["integrated"])) < 0.15, (loud, l2)
        peak1 = wave_peak(out / "norm.wav")
        assert abs(20 * math.log10(peak1 / peak0) - l2["gain"]) < 0.2, (peak0, peak1, l2)
        assert not (out / "norm.part.wav").exists()
        print(f"song edit: ok  (split, undo/redo incl. the take, export: {len(res['files'])} files of {secs:.2f} s "
              f"in {time.monotonic() - t0:.2f} s; {loud['integrated']} LUFS, normalized to {l2['integrated']})")
    finally:
        d.close()
        e.close()
        shutil.rmtree(root, ignore_errors=True)


def test_instruments():
    """A strip plays an instrument instead of an input: the built-in drum kit
    and synth answer notes, the partner gets them like any channel, and a song
    track records them. A VST3 instrument (engine/examples/test_synth) gets its
    notes and keeps its settings."""
    x = [0.0] * N
    a, b, _ = connect_pair()
    for e in (a, b):
        e.strip_set("L0", "mon", 1)
    a.inst_set("L0", "drums")
    assert a.mixer()["local"][0]["inst"]["kind"] == "drums"
    assert max(abs(v) for v in a.process(x)) == 0.0, "silent until played"
    a.midi_target("L0")
    a.note("L0", True, 36, 120)  # kick
    base = b.state()["packets_in"]
    heard, played = 0.0, 0.0
    for k in range(40):
        played = max(played, max(abs(v) for v in a.process(x)))
        wait_packets(b, base + k + 1)
        heard = max(heard, max(abs(v) for v in b.process(x)))
    assert played > 0.2 and heard > 0.2, (played, heard)
    # the kit's knobs by name: the kit itself, and every pad's own (an old song's
    # three knobs still load, the rest start at their defaults)
    a.inst_set("L0", "drums", None, {"level": 0.8, "tune": 0.5, "decay": 1.0})
    m = a.mixer()["local"][0]["inst"]
    assert sum(1 for k in m if re.fullmatch(r"p\d+_[a-z]+", k)) == 80 and "kit" in m and m["decay"] == 1.0, sorted(m)
    a.inst_param("L0", "p0_level", 0.0)
    a.inst_param("L0", "kit", 0.1)  # the 808
    a.note("L0", True, 36, 120)
    assert max(max(abs(v) for v in a.process(x)) for _ in range(10)) < 1e-3, "the kick's own level at 0"
    a.note("L0", True, 38, 120)
    assert max(max(abs(v) for v in a.process(x)) for _ in range(10)) > 0.1, "the snare still"
    # the synth (unison on), then back to the input
    a.inst_set("L0", "synth", None, {"cutoff": 0.9, "sub": 0.5})
    assert abs(a.mixer()["local"][0]["inst"]["cutoff"] - 0.9) < 1e-6 and "lfo_rate" in a.mixer()["local"][0]["inst"]
    a.note("L0", True, 60, 100)
    out = []
    for _ in range(30):
        out += a.process(x)
    spec = np.abs(np.fft.rfft(np.array(out[-2048:]) * np.hanning(2048)))
    f0 = np.argmax(spec[:200]) * SR / 2048
    assert abs(f0 - 261.6) < 25, f"middle C, not {f0:.0f} Hz"
    a.inst_param("L0", "unison", 1.0)  # 7 voices, wide
    out = []
    for _ in range(10):
        out += a.process(x)
    assert a.mixer()["local"][0]["inst"]["unison"] == 1.0 and max(abs(v) for v in out) > 0.05
    a.inst_set("L0", None)
    assert a.mixer()["local"][0]["inst"] is None
    a.close()
    b.close()

    # a drum part recorded onto a song track
    import tempfile
    from pathlib import Path
    e = Engine()
    e.inst_set("L1", "drums")
    e.strip_set("L1", "active", 1)
    d = tempfile.mkdtemp()
    e.daw_record([(1, "drumtake", str(Path(d) / "drums.wav"))], 0.0)
    for k in range(100):
        if k % 25 == 0:
            e.note("L1", True, 38, 110)
        e.process(x)
    take = e.daw_stop()[0]
    rec = wave_peak(Path(d) / "drums.wav")
    assert take["frames"] == 100 * N and rec > 0.2, (take["frames"], rec)
    e.close()

    path = test_plugin_path("test_synth")
    if not path:
        print("instruments: ok  (drums, synth, sent, recorded; VST instrument skipped: build examples/test_synth)")
        return
    cls = Engine.vst_scan(path)
    assert cls[0]["instrument"], cls
    v = Engine()
    v.strip_set("L0", "mon", 1)
    v.inst_set("L0", "vst", (path, cls[0]["cid"], cls[0]["name"]))
    v.note("L0", True, 69, 100)
    out = []
    for _ in range(20):
        out += v.process(x)
    spec = np.abs(np.fft.rfft(np.array(out[-2048:]) * np.hanning(2048)))
    assert abs(np.argmax(spec) * SR / 2048 - 440) < 25, "the plugin plays the note it was sent"
    v.note("L0", False, 69)
    v.process(x)
    assert max(abs(s) for s in v.process(x)) == 0.0, "and stops on note off"
    saved = v.mixer(True)["local"][0]["inst"]
    w = Engine()
    w.inst_set("L0", saved["kind"], None, saved)  # what restoring a saved mixer does
    assert w.mixer()["local"][0]["inst"]["vst"]["cid"] == cls[0]["cid"]
    v.close()
    w.close()
    print("instruments: ok  (drums, synth, sent to the partner, recorded on a track, VST instrument plays and restores)")


def test_patterns():
    """FL-style patterns: a channel (a track with the drum kit), a pattern with
    a closed hat on every beat, one clip twice its length (it repeats). Both sides
    play the same kicks at the same samples; notes played while recording turn
    into a new pattern; pattern mode plays one pattern looped."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw

    a, b, _ = connect_pair()
    root = Path(tempfile.mkdtemp(prefix="duet-pat-"))
    da, db = Daw(a, root / "A", "alice"), Daw(b, root / "B", "bob")
    x = [0.0] * N
    try:
        wait_for(lambda: da.p.pid == db.p.pid and da.view()["shared"] and db.view()["shared"], what="one shared song")
        da.edit([["meta", "meta", {"bpm": 120.0}],
                 ["track", "drums", {"name": "Drums", "order": 5.0, "owner": "alice", "vol": 1.0, "pan": -1.0,
                                     "inst": {"kind": "drums", "level": 0.8, "tune": 0.5, "decay": 1.0}}],
                 ["pattern", "p1", {"name": "Beat", "len": 4.0}]]
                + [["note", f"n{k}", {"pattern": "p1", "track": "drums", "start": float(k), "len": 0.25, "key": 42, "vel": 120}]
                   for k in range(4)]
                + [["clip", "pc", {"track": "drums", "pattern": "p1", "start": 0.0, "blen": 8.0, "boff": 0.0}]])
        wait_for(lambda: len(db.view()["notes"]) == 4 and db.view()["patterns"] and
                 any(t.get("inst") for t in db.view()["tracks"]), what="pattern on bob's side")
        onsets = []
        for d, e in ((da, a), (db, b)):
            d.set_local("click", False)
            e.daw_play(0.0)
            out = []
            for _ in range(int(4.2 * SR / N)):  # 8 beats at 120 bpm + a bit
                out += e.process(x)
            e.daw_stop()
            onsets.append(hits_in(out))
        spb = SR // 2  # samples per beat at 120 bpm
        assert onsets[0] == onsets[1], f"both sides play the same: {onsets}"
        assert len(onsets[0]) == 8 and all(abs(h - k * spb) < 4 for k, h in enumerate(onsets[0])), onsets[0]

        # record: arm the channel, play three notes, and they become a pattern + clip
        da.edit([["track", "drums", {"arm": True}]])
        da.set_local("countin", False)
        assert da.record(0.0) == {"ok": True}
        slot = da.slots["drums"]
        for k in range(200):
            if k in (10, 60, 110):
                a.note(f"T{slot}", True, 38, 100)
            if k in (20, 70, 120):
                a.note(f"T{slot}", False, 38)
            a.process(x)
        da.stop()
        v = da.view()
        takes = [p for p in v["patterns"] if p["id"] != "p1"]
        assert len(takes) == 1, v["patterns"]
        mine = sorted((n["start"], n["key"]) for n in v["notes"] if n["pattern"] == takes[0]["id"])
        assert [k for _, k in mine] == [38, 38, 38] and abs(mine[1][0] - mine[0][0] - 50 * N / spb) < 0.02, mine
        assert any(c.get("pattern") == takes[0]["id"] for c in v["clips"]), "placed in the playlist"
        wait_for(lambda: len(db.view()["patterns"]) == 2, what="the recorded pattern on bob's side")

        # pattern mode: just "Beat", looped, whatever else is in the song (once
        # the snares played above have rung out)
        for _ in range(int(4 * SR / N)):
            if max(abs(v) for v in a.process(x)) < 1e-3:
                break
        da.play_pattern("p1")
        assert da.view()["patternMode"] == "p1"
        out = []
        for _ in range(int(2.5 * SR / N)):
            out += a.process(x)
        da.stop()
        assert da.view()["patternMode"] is None
        hits = hits_in(out)
        assert len(hits) == 5 and abs(hits[4] - 4 * spb) < 4, f"loops after 4 beats: {hits}"
        print("patterns: ok  (channel + pattern + repeating clip play the same on both sides, to the sample; "
              "recorded notes become a pattern; pattern mode loops)")
    finally:
        da.close()
        db.close()
        a.close()
        b.close()
        shutil.rmtree(root, ignore_errors=True)


def test_live():
    """Working side by side. Notes played live on a song track reach the same
    track on the partner's side, whatever slot it sits in there, and play through
    their copy of its instrument. They aren't the partner's to record, a lossy
    line doesn't leave notes hanging, nor does the line dropping mid-note. And
    each side sees where the other is working (presence)."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw

    x = [0.0] * N
    a, b, _ = connect_pair()
    for e, slot in ((a, 2), (b, 7)):
        e.strip_set(f"T{slot}", "active", 1)
        e.inst_set(f"T{slot}", "synth")
    a.track_keys([0, 0, 4242])
    b.track_keys([0] * 7 + [4242])
    a.note("T2", True, 60, 100)
    wait_for(lambda: b.partner_notes == [(7, 60)], what="alice's note on bob's side", detail=lambda: b.partner_notes)
    out = []
    for _ in range(30):
        out += b.process(x)
    spec = np.abs(np.fft.rfft(np.array(out[-2048:]) * np.hanning(2048)))
    f0 = np.argmax(spec[:200]) * SR / 2048
    assert abs(f0 - 261.6) < 25, f"bob hears middle C on his track, not {f0:.0f} Hz"
    a.note("T2", False, 60)
    wait_for(lambda: b.partner_notes == [], what="the note let go")

    # bob records: alice's live notes are hers to record, his own are his
    b.daw_record([], 0.0)
    a.note("T2", True, 64, 100)
    wait_for(lambda: b.partner_notes == [(7, 64)], what="a note while bob records")
    b.note("T7", True, 50, 90)
    for _ in range(5):
        b.process(x)
    a.note("T2", False, 64)
    b.note("T7", False, 50)
    wait_for(lambda: b.partner_notes == [], what="the note let go")
    b.process(x)
    b.daw_stop()
    rec = b.daw_rec_notes()
    assert [(t, kind, key) for t, kind, key, _v, _p in rec] == [(7, "on", 50), (7, "off", 50)], rec

    # a third of the note packets lost: every note still ends
    a.set_test_signal_loss(0.33)
    for k in range(60):
        a.note("T2", True, 40 + k % 24, 90)
        time.sleep(0.002)
        a.note("T2", False, 40 + k % 24)
    a.set_test_signal_loss(0.0)
    wait_for(lambda: b.partner_notes == [], what="no hanging notes after loss", detail=lambda: b.partner_notes)

    # the line drops mid-note
    a.note("T2", True, 67, 100)
    wait_for(lambda: b.partner_notes == [(7, 67)], what="a held note")
    a.close()
    wait_for(lambda: b.partner_notes == [], what="the partner's notes let go when the line drops")
    b.close()

    # through the song: both sides know a track by its id
    c, d, _ = connect_pair()
    root = Path(tempfile.mkdtemp(prefix="duet-live-"))
    dc, dd = Daw(c, root / "A", "alice"), Daw(d, root / "B", "bob")
    try:
        wait_for(lambda: dc.p.pid == dd.p.pid and dc.view()["shared"] and dd.view()["shared"], what="one shared song")
        dc.edit([["track", "keys", {"name": "Keys", "order": 1.0, "owner": "alice",
                                    "inst": {"kind": "synth", "cutoff": 0.6}}]])
        wait_for(lambda: "keys" in dd.slots, what="the channel on bob's side")
        c.note(f"T{dc.slots['keys']}", True, 62, 100)
        wait_for(lambda: dd.status(d.state())["partnerNotes"] == {"keys": [62]}, what="bob sees alice play Keys",
                 detail=lambda: dd.status(d.state())["partnerNotes"])
        c.note(f"T{dc.slots['keys']}", False, 62)

        here = {"view": "seq", "pattern": "p9", "track": "keys", "sel": ["c1", "../x"], "notes": ["n1"],
                "ptr": {"in": "roll", "beat": 1.5, "key": 62}, "roll": {"beat": 0.0, "key": 72}}
        dc.set_presence(here)
        wait_for(lambda: dd.status(d.state())["partnerHere"], what="alice's presence on bob's side")
        h = dd.status(d.state())["partnerHere"]
        assert h["view"] == "seq" and h["track"] == "keys" and h["ptr"] == {"in": "roll", "beat": 1.5, "key": 62}, h
        assert h["sel"] == ["c1"], "only plain ids get through"
        dd._on_msg({"app": "daw", "pid": dd.p.pid, "t": "here", "n": 1.0, "view": "mixer"})
        assert dd.status(d.state())["partnerHere"]["view"] == "seq", "an older message doesn't win"
        assert dc.status(c.state())["partnerHere"] is None, "bob hasn't said where he is"
        print("live: ok  (notes played live reach the partner's track and instrument, not their recording; "
              "nothing hangs after loss or a dropped line; presence both ways)")
    finally:
        dc.close()
        dd.close()
        c.close()
        d.close()
        shutil.rmtree(root, ignore_errors=True)


def test_routing():
    """The song's mixer, through the shared song: a channel routed to a bus
    goes through that bus (its fader), a send adds to a bus, the master rack (a
    limiter) holds its ceiling, and the partner's mixer follows all of it."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw

    a, b, _ = connect_pair()
    root = Path(tempfile.mkdtemp(prefix="duet-mix-"))
    da, db = Daw(a, root / "A", "alice"), Daw(b, root / "B", "bob")
    x = [0.0] * N

    def peak(e, blocks=60):
        e.daw_play(0.0)
        out = []
        for _ in range(blocks):
            out += e.process(x)
        e.daw_stop()
        return max(abs(v) for v in out[len(out) // 2:])

    try:
        wait_for(lambda: da.p.pid == db.p.pid and da.view()["shared"] and db.view()["shared"], what="one shared song")
        db.set_local("click", False)
        da.edit([["track", "keys", {"name": "Keys", "order": 1.0, "owner": "alice", "vol": 1.0,
                                    "inst": {"kind": "synth", "cutoff": 0.7}}],
                 ["track", "grp", {"name": "Group", "order": 1001.0, "owner": "alice", "bus": True, "vol": 0.0}],
                 ["pattern", "p1", {"name": "Hold", "len": 8.0}],
                 ["note", "n1", {"pattern": "p1", "track": "keys", "start": 0.0, "len": 8.0, "key": 60, "vel": 110}],
                 ["clip", "c1", {"track": "keys", "pattern": "p1", "start": 0.0, "blen": 8.0}]])
        wait_for(lambda: {"keys", "grp"} <= set(db.slots) and len(db.view()["notes"]) == 1, what="the song on bob's side")
        assert [t["id"] for t in db.view()["buses"]] == ["grp"] and "grp" not in [t["id"] for t in db.view()["tracks"]]
        loud = peak(b)
        assert loud > 0.05, f"the channel plays straight to the master: {loud}"

        # into the bus, whose fader is down: silence; up: there again
        da.edit([["track", "keys", {"out": "grp"}]])
        wait_for(lambda: (db.sent_routes or [])[db.slots["keys"]][0] == db.slots["grp"], what="the route on bob's side")
        assert peak(b) < 1e-4, "through the bus, fader down"
        da.edit([["track", "grp", {"vol": 1.0}]])
        wait_for(lambda: b.mixer()["tracks"][db.slots["grp"]]["fader"] == 1.0, what="the bus fader on bob's side")
        assert abs(peak(b) - loud) < 0.02 * loud + 1e-3, "the bus at unity: as loud as before"

        # a send from the channel (back on the master) to a bus adds to it
        da.edit([["track", "keys", {"out": None, "sends": {"grp": 1.0}}]])
        wait_for(lambda: (db.sent_routes or [])[db.slots["keys"]] == (-1, [(db.slots["grp"], 1.0)]), what="the send on bob's side")
        assert peak(b) > 1.6 * loud, "the dry sound plus the same again through the bus"

        # a live strip sends to the bus too (just on bob's side): his synth, not
        # monitored, is heard only through the bus while it's sent there
        def live_peak(blocks=40):
            out = []
            for _ in range(blocks):
                out += b.process(x)
            return max(abs(v) for v in out[len(out) // 2:])

        live_peak(240)  # the song's tails ring out first
        b.strip_set("L0", "mon", 0)
        b.inst_set("L0", "synth")
        b.note("L0", True, 60, 110)
        assert live_peak() < 1e-4, "not monitored, not sent: silent"
        db.set_local("livesends", {"grp": 1.0}, "L0")
        wait_for(lambda: db.sent_live.get("L0") == [(db.slots["grp"], 1.0)], what="the live send in bob's engine")
        sent = live_peak()
        assert sent > 0.02, f"bob's synth through the bus: {sent}"
        db.set_local("livesends", {}, "L0")
        wait_for(lambda: db.sent_live.get("L0") == [], what="the live send gone")
        b.note("L0", False, 60)
        assert live_peak(80) < 1e-3, "and silent again"
        assert "L0" not in (da.view()["local"]["livesends"]), "a live send is bob's alone"
        b.inst_set("L0", None)

        # the master rack: a limiter at -12 dB, 24 dB of gain into it
        da.edit([["meta", "meta", {"master_rack": [{"kind": "limiter", "on": True, "gain": 1.0, "ceiling": 0.0, "release": 0.4}]}]])
        wait_for(lambda: [s["kind"] for s in b.mixer()["master_strip"]["rack"]] == ["limiter"], what="the master rack on bob's side")
        assert b.state()["master_latency_ms"] > 1.0, "the limiter looks ahead"
        held = peak(b)
        assert 0.2 < held <= 10 ** (-12 / 20) + 1e-4, f"at the ceiling, not over: {held}"
        print(f"routing: ok  (channel -> bus -> master, a send, a live strip's send ({sent:.2f}), "
              f"the master limiter at its ceiling: {held:.3f}; all on the partner's side)")
    finally:
        da.close()
        db.close()
        a.close()
        b.close()
        shutil.rmtree(root, ignore_errors=True)


def test_automation():
    """Automation lanes in the shared song: a fader lane fades a track out (on
    the partner's side too, and in an export), a lane taken away gives the
    fader back, an effect knob follows its lane, and a lane whose effect isn't
    there says so."""
    import shutil
    import tempfile
    import wave
    from pathlib import Path

    from daw import Daw

    a, b, _ = connect_pair()
    root = Path(tempfile.mkdtemp(prefix="duet-auto-"))
    da, db = Daw(a, root / "A", "alice"), Daw(b, root / "B", "bob")
    x = [0.0] * N

    def peak(e, at=0.0, blocks=60):
        e.daw_play(at)
        out = []
        for _ in range(blocks):
            out += e.process(x)
        e.daw_stop()
        return max(abs(v) for v in out[len(out) // 2:])

    try:
        wait_for(lambda: da.p.pid == db.p.pid and da.view()["shared"] and db.view()["shared"], what="one shared song")
        for d in (da, db):
            d.set_local("click", False)
        da.edit([["meta", "meta", {"bpm": 120.0}],
                 ["track", "keys", {"name": "Keys", "order": 1.0, "owner": "alice", "vol": 1.0,
                                    "inst": {"kind": "synth", "cutoff": 0.7}, "rack": [{"kind": "drive", "level": 0.5}]}],
                 ["pattern", "p1", {"name": "Hold", "len": 16.0}],
                 *[["note", f"n{i}", {"pattern": "p1", "track": "keys", "start": i * 0.25, "len": 0.25, "key": 60, "vel": 110}]
                   for i in range(64)],  # short notes: playing from anywhere starts one
                 ["clip", "c1", {"track": "keys", "pattern": "p1", "start": 0.0, "blen": 16.0}]])
        wait_for(lambda: "keys" in db.slots and len(db.view()["notes"]) == 64, what="the song on bob's side")
        loud = peak(b, 1.0)
        assert loud > 0.05, loud

        # the fader lane: unity for 4 beats (2 s), then down to nothing at beat 8 (4 s)
        unity = 0.7937
        da.edit([["track", "a1", {"name": "Keys volume", "order": 1.5, "owner": "alice",
                                  "auto": {"target": "keys", "param": "vol"},
                                  "points": [[0.0, unity], [4.0, unity], [8.0, 0.0]]}]])
        wait_for(lambda: db.sent_autos and db.sent_autos[0], what="the lane on bob's side")
        assert "a1" not in db.slots, "a lane has no mixer strip"
        assert "a1" in [t["id"] for t in db.view()["tracks"]] and not db.view()["tracks"][1].get("autoBad")
        assert abs(peak(b, 1.0) - loud) < 0.05 * loud, "at unity: as before"
        mid = peak(b, 3.0)  # beat 6: halfway down
        assert 0.02 * loud < mid < 0.6 * loud, (mid, loud)
        assert peak(b, 4.5) < 1e-4, "faded out"

        # in an export too: the end of the mix is silent
        out = root / "out"
        out.mkdir()
        da.edit([["meta", "meta", {"loop": True, "loop_start": 0.0, "loop_end": 12.0}]])
        res = da.export(str(out / "mix.wav"), None, loop=True)
        assert res.get("ok"), res
        t0 = time.monotonic()
        while (st := a.daw_export_status())["result"] is None:
            assert time.monotonic() - t0 < 20, "export never finished"
            time.sleep(0.02)
        assert st["result"]["ok"], st
        with wave.open(str(out / "mix.wav")) as w:
            sr, raw = w.getframerate(), w.readframes(w.getnframes())
        v = np.frombuffer(b"".join(raw[i:i + 3] + (b"\xff" if raw[i + 2] & 0x80 else b"\x00")
                                   for i in range(0, len(raw), 3)), dtype="<i4") / 8388608.0
        first, last = np.abs(v[sr // 2 * 2:sr * 2 * 2]).max(), np.abs(v[int(4.5 * sr) * 2:]).max()
        assert first > 0.05 and last < 1e-3, (first, last)

        # the lane gone: the fader's own setting again
        da.edit([["track", "a1", {"del": True}]])
        wait_for(lambda: db.sent_autos and not db.sent_autos[0], what="the lane gone on bob's side")
        assert abs(peak(b, 4.5) - loud) < 0.05 * loud, "the fader is back"

        # an effect knob: the drive's level all the way down (0.2x instead of 0.85x)
        da.edit([["track", "a2", {"name": "Drive level", "order": 1.5, "owner": "alice",
                                  "auto": {"target": "keys", "param": "fx:0:drive:level"}, "points": [[0.0, 0.0]]}]])
        wait_for(lambda: db.sent_autos and db.sent_autos[0], what="the knob lane on bob's side")
        assert peak(b, 1.0) < 0.35 * loud, "the drive's level follows its lane"
        assert b.mixer()["tracks"][db.slots["keys"]]["rack"][0]["level"] == 0.5, "the knob itself keeps its setting"
        # pointing at an effect that isn't there
        da.edit([["track", "a2", {"auto": {"target": "keys", "param": "fx:0:reverb:mix"}}]])
        wait_for(lambda: [t for t in db.view()["tracks"] if t["id"] == "a2"][0].get("autoBad"), what="the lane flagged")
        assert abs(peak(b, 1.0) - loud) < 0.05 * loud, "and the drive back as it was"
        print(f"automation: ok  (fade {loud:.3f} -> {mid:.3f} -> 0 on the partner's side and in the export)")
    finally:
        da.close()
        db.close()
        a.close()
        b.close()
        shutil.rmtree(root, ignore_errors=True)


def test_big_file():
    """A long file into the song: the project never stops answering while it
    loads (the UI asks it something every frame), and a 44.1k file plays at the
    right pitch on a 48k device (converted as it plays)."""
    import shutil
    import tempfile
    import threading
    import wave
    from pathlib import Path

    from daw import Daw

    e = Engine()
    e.start()
    root = Path(tempfile.mkdtemp(prefix="duet-big-"))
    d = Daw(e, root / "P", "me")
    try:
        sr, secs, hz = 44100, 240, 1000.0
        src = root / "long.wav"
        with wave.open(str(src), "wb") as w:
            w.setnchannels(2)
            w.setsampwidth(2)
            w.setframerate(sr)
            for k in range(secs // 10):
                t = np.arange(k * 10 * sr, (k + 1) * 10 * sr) / sr
                x = (0.5 * np.sin(2 * np.pi * hz * t) * 32767).astype("<i2")
                w.writeframes(np.repeat(x, 2).tobytes())
        worst, stop = [0.0], [False]

        def poll():
            while not stop[0]:
                t0 = time.monotonic()
                d.view()
                d.status(e.state())
                worst[0] = max(worst[0], time.monotonic() - t0)
                time.sleep(0.01)

        th = threading.Thread(target=poll)
        th.start()
        t0 = time.monotonic()
        res = d.sample_clip(str(src), None, 0.0)
        assert res.get("ok"), res
        key = d.view()["clips"][0]["file"]
        wait_for(lambda: key in d.loaded, timeout=60, what="the long file loaded")
        took = time.monotonic() - t0
        stop[0] = True
        th.join()
        assert worst[0] < 0.25, f"the project didn't answer for {worst[0]:.2f} s while loading"
        f = d.view()["files"][key]
        assert f["here"] and not f["loading"] and f["frames"] == secs * sr
        # zoomed in: the waveform of any stretch at any resolution (here 1 kHz at 44.1k: a peak of
        # about the tone's amplitude every column of 64 frames, and single samples at 0.5 a column)
        w = np.frombuffer(e.daw_wave(key, sr * 10.0, 64.0, 500), dtype="<i2").reshape(-1, 2)
        assert w.shape == (500, 2) and (w[:, 1] > 0.4 * 32767).all() and (w[:, 0] < -0.4 * 32767).all(), w[:3]
        s1 = np.frombuffer(e.daw_wave(key, sr * 10.0, 0.5, 40), dtype="<i2").reshape(-1, 2)
        assert (s1[:, 0] == s1[:, 1]).all() and (s1[0::2] == s1[1::2]).all()
        assert e.daw_wave("nothing.wav", 0.0, 1.0, 4) is None
        wait_for(lambda: d.sent_desc and any(d.sent_desc), what="the clip on the timeline")
        # 1 kHz in, 1 kHz out at the device's 48k: count the zero crossings
        e.daw_play(5.0)
        x = [0.0] * N
        out = []
        for _ in range(100):
            out += e.process(x)
        e.daw_stop()
        left = np.array(out[N * 10:])  # (the left channel)
        dev = e.state()["samplerate"]
        cross = np.count_nonzero(np.diff(np.signbit(left)))
        got = cross / 2 / (len(left) / dev)
        assert abs(got - hz) < 5, f"plays at {got:.1f} Hz instead of {hz}"
        print(f"big file: ok  ({secs // 60} min stereo at 44.1k loaded in {took:.1f} s, longest wait for an answer "
              f"{worst[0] * 1000:.0f} ms, plays at {got:.0f} Hz on {dev} Hz)")
    finally:
        d.close()
        e.close()
        shutil.rmtree(root, ignore_errors=True)


def test_clip_edit():
    """The clip editor's changes, through the shared song: tape speed (faster
    and higher), keep pitch (faster, same pitch: a stretched copy made in the
    background), transpose, backwards; the partner hears the same, an export
    waits for a stretched copy it needs; and the analysis finds a beat's tempo,
    its hits and the peak."""
    import shutil
    import tempfile
    import wave
    from pathlib import Path

    from daw import Daw

    a, b, _ = connect_pair()
    root = Path(tempfile.mkdtemp(prefix="duet-clip-"))
    da, db = Daw(a, root / "A", "alice"), Daw(b, root / "B", "bob")
    x = [0.0] * N

    def write(path, samples, sr=48000):
        with wave.open(str(path), "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(sr)
            w.writeframes((np.clip(samples, -1, 1) * 32767).astype("<i2").tobytes())

    def listen(e, at, blocks=80):
        e.daw_play(at)
        out = []
        for _ in range(blocks):
            out += e.process(x)
        e.daw_stop()
        return np.array(out[N * 16:])

    def hz(sig):
        dev = 48000
        return np.count_nonzero(np.diff(np.signbit(sig))) / 2 / (len(sig) / dev)

    try:
        wait_for(lambda: da.p.pid == db.p.pid and da.view()["shared"] and db.view()["shared"], what="one shared song")
        for d in (da, db):
            d.set_local("click", False)
        da.edit([["meta", "meta", {"bpm": 120.0}]])
        t = np.arange(48000 * 6) / 48000
        sine = root / "sine.wav"
        write(sine, 0.5 * np.sin(2 * np.pi * 500 * t))
        res = da.sample_clip(str(sine), None, 0.0)
        assert res.get("ok"), res
        cid = da.view()["clips"][0]["id"]
        key = da.view()["clips"][0]["file"]
        wait_for(lambda: key in db.loaded and db.sent_desc and any(db.sent_desc), timeout=20, what="the clip on bob's side")
        base = hz(listen(b, 0.5))
        assert abs(base - 500) < 5, base

        def set_clip(fields, what):
            da.edit([["clip", cid, fields]])
            wait_for(lambda: all(db.p.live("clip")[cid].get(k) == v for k, v in fields.items()), what=what)
            wait_for(lambda: key not in b.daw_processing(), timeout=30, what=f"{what}: stretched")

        # tape: twice as fast is an octave up
        set_clip({"speed": 2.0, "len": 3.0}, "tape 2x")
        tape = hz(listen(b, 0.5))
        assert abs(tape - 1000) < 10, tape
        # keep pitch: twice as fast, same pitch
        set_clip({"stretch": True}, "keep pitch 2x")
        keep = hz(listen(b, 0.5))
        assert abs(keep - 500) < 8, keep
        # transposed a fifth up, at the original speed
        set_clip({"speed": 1.0, "len": 6.0, "pitch": 7.0}, "a fifth up")
        fifth = hz(listen(b, 0.5))
        assert abs(fifth - 500 * 2 ** (7 / 12)) < 8, fifth
        # backwards: a file that gets louder is a clip that gets softer
        ramp = root / "ramp.wav"
        write(ramp, np.linspace(0, 0.9, 48000 * 2) * np.sin(2 * np.pi * 300 * t[:96000]))
        res = da.sample_clip(str(ramp), None, 16.0)  # (at 8 s: after the sine's 6)
        rid = [c for c in da.view()["clips"] if c["id"] != cid][0]["id"]
        rkey = da.p.live("clip")[rid]["file"]
        da.edit([["clip", rid, {"reverse": True}]])
        wait_for(lambda: rkey in db.loaded and db.p.live("clip").get(rid, {}).get("reverse"), timeout=20, what="the backwards clip")
        wait_for(lambda: db.sent_desc and any(len(t) > 0 and any(c[0] == rkey and c[9] for c in t) for t in db.sent_desc), what="backwards on bob's timeline")
        rev = listen(b, 8.0 + 0.05, blocks=48000 * 2 // N + 20)
        first, last = np.abs(rev[:4800]).max(), np.abs(rev[int(48000 * 1.75):int(48000 * 1.85)]).max()
        assert first > 4 * last, (first, last)

        # an export waits for a stretched copy it needs
        da.edit([["clip", cid, {"pitch": 0.0, "speed": 1.5, "len": 4.0}]])
        out = root / "out"
        out.mkdir()
        res = da.export(str(out / "mix.wav"), None, loop=False)
        assert res.get("ok"), res
        t0 = time.monotonic()
        while (st := a.daw_export_status())["result"] is None:
            assert time.monotonic() - t0 < 60, "export never finished"
            time.sleep(0.02)
        with wave.open(str(out / "mix.wav")) as w:
            raw = w.readframes(w.getnframes())
        v = np.frombuffer(b"".join(raw[i:i + 3] + (b"\xff" if raw[i + 2] & 0x80 else b"\x00")
                                   for i in range(0, 48000 * 2 * 3 * 2, 3)), dtype="<i4") / 8388608.0
        assert np.abs(v[48000:]).max() > 0.2, "the stretched clip is in the export"

        # analysis: clicks at 120 BPM
        clicks = np.zeros(48000 * 8)
        for k in range(16):
            s0 = int((0.25 + 0.5 * k) * 48000)
            clicks[s0:s0 + 400] = 0.8 * np.exp(-np.arange(400) / 80) * np.where(np.arange(400) % 2, -1, 1)
        cl = root / "clicks.wav"
        write(cl, clicks)
        da.sample_clip(str(cl), None, 32.0)
        kid = [c for c in da.view()["clips"] if c["id"] not in (cid, rid)][0]["id"]
        wait_for(lambda: da.p.live("clip")[kid]["file"] in da.loaded, what="the clicks loaded")
        bpm = da.analyze(kid, "tempo")["bpm"]
        hits = da.analyze(kid, "hits", 0.5)["hits"]
        peak = da.analyze(kid, "peak")["peak"]
        assert abs(bpm - 120) < 1.5 and len(hits) == 16 and abs(peak - 0.8) < 0.01, (bpm, len(hits), peak)
        print(f"clip edit: ok  (tape 2x {tape:.0f} Hz, keep pitch 2x {keep:.0f} Hz, a fifth up {fifth:.0f} Hz, "
              f"backwards, export waits for the stretch; tempo {bpm} BPM, {len(hits)} hits)")
    finally:
        da.close()
        db.close()
        a.close()
        b.close()
        shutil.rmtree(root, ignore_errors=True)


def test_stems():
    """Splitting a clip into stems, through the shared song: four new tracks
    under its own, each with a clip in the same place, the clip itself muted,
    the stems adding back up to the clip; the partner gets the stem files and
    hears the same. Needs the model: DUET_STEM_MODEL=path/to/htdemucs.onnx
    (CI downloads it from the models-htdemucs-1 release); without it, only that
    the split asks for it."""
    import os
    import shutil
    import tempfile
    import wave
    from pathlib import Path

    from daw import STEM_MODEL, Daw

    a, b, _ = connect_pair()
    root = Path(tempfile.mkdtemp(prefix="duet-stems-"))
    da, db = Daw(a, root / "A", "alice"), Daw(b, root / "B", "bob")
    try:
        wait_for(lambda: da.p.pid == db.p.pid and da.view()["shared"] and db.view()["shared"], what="one shared song")
        sr = 44100
        t = np.arange(int(sr * 9.0)) / sr
        kick = np.zeros_like(t)
        for k in np.arange(0, 9.0, 0.5):
            i, n = int(k * sr), int(0.2 * sr)
            tt = np.arange(n) / sr
            kick[i:i + n] += np.sin(2 * np.pi * (50 + 80 * np.exp(-tt / 0.03)) * tt) * np.exp(-tt / 0.1)
        mix = 0.4 * kick + 0.3 * np.sin(2 * np.pi * 55 * t) + 0.15 * np.sin(2 * np.pi * 440 * t) * (np.sin(2 * np.pi * 2 * t) > 0)
        src = root / "mix.wav"
        with wave.open(str(src), "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(sr)
            w.writeframes((np.clip(mix, -1, 1) * 32767).astype("<i2").tobytes())
        res = da.sample_clip(str(src), None, 0.0)
        cid = da.view()["clips"][0]["id"]
        key = da.view()["clips"][0]["file"]
        wait_for(lambda: key in da.loaded, what="the clip loaded")
        if not da.stems_available():
            r = da.split_stems(cid, "4")
            assert r.get("unavailable"), r
            print("stems: ok  (not in this build: the split says so)")
            return
        model = os.environ.get("DUET_STEM_MODEL")
        if not model or not Path(model).is_file():
            r = da.split_stems(cid, "4")
            assert r.get("need_model"), r
            print("stems: ok  (no model here: the split asks for it; set DUET_STEM_MODEL to run it)")
            return
        da.models_dir.mkdir(parents=True, exist_ok=True)
        os.symlink(model, da.stem_model())
        t0 = time.monotonic()
        r = da.split_stems(cid, "4")
        assert r.get("ok"), r
        wait_for(lambda: da.stem_job is None, timeout=600, what="the split")
        took = time.monotonic() - t0
        v = da.view()
        stems = [c for c in v["clips"] if c["id"] != cid]
        assert len(stems) == 4 and da.p.live("clip")[cid].get("mute"), (len(stems), da.p.live("clip")[cid])
        names = sorted(tr["name"] for tr in v["tracks"] if tr["id"] in {c["track"] for c in stems})
        assert names == sorted(f"mix · {n}" for n in ("Drums", "Bass", "Other", "Vocals")), names
        assert all(abs(c["start"] - 0.0) < 1e-9 and abs(c["len"] - 9.0) < 0.01 and c["offset"] == 0.0 for c in stems)
        def left(path):  # a 32-bit float stereo WAV's left side (Python's wave can't read float)
            raw = Path(path).read_bytes()
            at = raw.index(b"data") + 8
            return np.frombuffer(raw[at:at + int.from_bytes(raw[at - 4:at], "little")], dtype="<f4").reshape(-1, 2)[:, 0]

        # the stems add back up to the mix (that's what the model is made to do)
        total = None
        for c in stems:
            x = left(da.p.audio_dir / c["file"])
            total = x if total is None else total + x
        err = np.abs(total[:len(mix)] - mix[:len(total)]).max()
        assert err < 0.05, f"the stems add up to the mix within {err}"
        drums = [c for c in stems if "drums" in c["file"]][0]
        d = left(da.p.audio_dir / drums["file"])
        # the drums stem has the kick: loud where the kick hits, quiet between
        on = np.abs(d[int(0.5 * sr):int(0.55 * sr)]).max()
        off = np.abs(d[int(0.35 * sr):int(0.45 * sr)]).max()
        assert on > 3 * off, (on, off)
        # bob gets the stems and plays them
        wait_for(lambda: all(c["file"] in db.loaded for c in stems), timeout=120, what="the stems on bob's side")
        print(f"stems: ok  (9 s split into 4 in {took:.1f} s, they add up to the mix within {err:.3f}, the kick in the drums; "
              f"bob has them)")
    finally:
        da.close()
        db.close()
        a.close()
        b.close()
        shutil.rmtree(root, ignore_errors=True)


def test_tune():
    """The Tune tab's engine side, through the shared song: the song's key
    found from the other clips (a backing in A minor), a voice's notes found
    with their centres, notes moved into the key (as Snap to key does), and
    the partner hearing the voice in tune (its notes measured again)."""
    import shutil
    import tempfile
    import wave
    from pathlib import Path

    from daw import Daw

    a, b, _ = connect_pair()
    root = Path(tempfile.mkdtemp(prefix="duet-tune-"))
    da, db = Daw(a, root / "A", "alice"), Daw(b, root / "B", "bob")
    sr = 44100

    def write(path, x):
        with wave.open(str(path), "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(sr)
            w.writeframes((np.clip(x, -1, 1) * 32767).astype("<i2").tobytes())

    def tone(midi, secs, harmonics=8, vib=0.0):
        t = np.arange(int(secs * sr)) / sr
        m = midi + vib * np.sin(2 * np.pi * 5.5 * t)
        ph = np.cumsum(2 * np.pi * 440 * 2 ** ((m - 69) / 12) / sr)
        env = np.minimum(1, t / 0.02) * np.minimum(1, (secs - t) / 0.02)
        return 0.3 * env * sum(np.sin(ph * h) / h for h in range(1, harmonics + 1))

    try:
        wait_for(lambda: da.p.pid == db.p.pid and da.view()["shared"] and db.view()["shared"], what="one shared song")
        for d in (da, db):
            d.set_local("click", False)
        # the backing: Am, F, C, G (in A minor), two seconds each
        chords = [[57, 60, 64], [53, 57, 60], [48, 52, 55], [55, 59, 62]]
        backing = np.concatenate([sum(tone(m, 2.0, 5) for m in ch) / 3 for ch in chords] * 2)
        write(root / "backing.wav", backing)
        # the voice: A4 +35 cents, C5 -30, E5 +8 (in tune enough), D5 +45, with vibrato
        plan = [(69.35, 0.7), (71.7, 0.7), (76.08, 0.7), (74.45, 0.7)]
        gap = np.zeros(int(0.15 * sr))
        voice = np.concatenate([np.concatenate([tone(m, d, 10, 0.15), gap]) for m, d in plan])
        write(root / "voice.wav", voice)
        da.sample_clip(str(root / "backing.wav"), None, 0.0)
        da.sample_clip(str(root / "voice.wav"), None, 0.0)
        clips = da.view()["clips"]
        vid = [c for c in clips if c["file"] != clips[0]["file"]][0]["id"] if len(clips) == 2 else None
        vid = [c["id"] for c in clips if da.view()["files"][c["file"]].get("name") == "voice"][0]
        wait_for(lambda: all(c["file"] in da.loaded for c in da.view()["clips"]), what="both loaded")
        key = da.analyze(vid, "key")
        # (C major has the same notes: either is right for tuning)
        assert key["key"] in ("A minor", "C major") and set([key["key"]] + key["others"][:1]) == {"A minor", "C major"}, key
        p = da.analyze(vid, "pitch")["pitch"]
        notes = p["notes"]
        assert len(notes) == 4, notes
        assert all(abs(n[2] - want) < 0.08 for n, (want, _) in zip(notes, plan)), [round(n[2], 2) for n in notes]
        # dragging a note: you hear just that note, moved (to you alone)
        s0, e0, c0 = notes[0]
        assert da.tune_audition(vid, s0, e0, c0, 2.0) == {"ok": True}
        a.process([0.0] * N)
        pos, total = a.preview_state
        assert total > int((e0 - s0) * 48000 * 0.9), (pos, total)
        a.preview(None)
        # Snap to key, full strength, leaving notes within 12 cents
        scale = {9, 11, 0, 2, 4, 5, 7}
        moves = []
        for s0, e0, c0 in notes:
            target = min((m for m in range(int(c0) - 2, int(c0) + 3) if m % 12 in scale), key=lambda m: abs(m - c0))
            if abs(target - c0) * 100 > 12:
                moves.append([s0, e0, c0, target - c0, 0.0])
        assert len(moves) == 3, moves
        da.edit([["clip", vid, {"tune": {"key": "A minor", "notes": moves}}]])
        vkey = da.p.live("clip")[vid]["file"]
        wait_for(lambda: vkey in db.loaded and db.p.live("clip").get(vid, {}).get("tune"), timeout=30, what="the tune on bob's side")
        wait_for(lambda: vkey not in b.daw_processing() and any(len(c) > 10 and c[10] for t in (db.sent_desc or []) for c in t),
                 timeout=30, what="bob's tuned voice made")
        time.sleep(0.3)
        # bob plays the song: just the voice (backing muted), measured again
        bk = [c["id"] for c in db.view()["clips"] if c["id"] != vid][0]
        db.edit([["clip", bk, {"mute": True}]])
        wait_for(lambda: db.sent_desc and all(c[4] == 0.0 for t in db.sent_desc for c in t if c[0] != vkey), what="backing muted")
        b.daw_play(0.0)
        x = [0.0] * N
        out = []
        for _ in range(int(3.6 * 48000 / N)):
            out += b.process(x)
        b.daw_stop()
        heard = root / "heard.wav"
        with wave.open(str(heard), "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(48000)
            w.writeframes((np.clip(np.array(out), -1, 1) * 32767).astype("<i2").tobytes())
        b.daw_load("heard", str(heard))
        got = [round(n[2], 2) for n in b.daw_pitch("heard", 0.0, 3.6)["notes"]]
        assert len(got) == 4, got
        want = [69.0, 72.0, 76.08, 74.0]
        assert all(abs(g - w) < 0.08 for g, w in zip(got, want)), (got, want)
        print(f"tune: ok  (key {key['key']} from the backing, notes {[round(n[2], 2) for n in notes]} -> heard on bob's side {got})")
    finally:
        da.close()
        db.close()
        a.close()
        b.close()
        shutil.rmtree(root, ignore_errors=True)


def test_library():
    """The sample library: the built-in sounds as WAVs, folders you make, sounds
    you import, search, nested bins; nothing outside its folders. And samples
    into the song: a clip, a sampler channel, a drum pad; the partner gets the
    files and plays the same."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw
    from library import Library

    root = Path(tempfile.mkdtemp(prefix="duet-lib-"))
    try:
        lib = Library(root / "Library", Engine)
        kit = lib.listing(str(lib.sounds / "Drum kit"))
        assert len(kit["files"]) == 16 and kit["files"][0]["name"] == "36 Kick", kit["files"][:2]
        kick = kit["files"][0]["path"]
        assert not kit["writable"], "the built-in sounds stay as they are"
        f = lib.new_folder(str(lib.mine), "Drums/../x")
        assert f["ok"] and Path(f["path"]).parent == lib.mine.resolve(), f
        assert lib.new_folder(str(lib.sounds), "nope").get("error")
        assert lib.listing(str(root)).get("error") and lib.listing("/").get("error"), "only the library's folders"
        imp = lib.import_files([kick, kick], f["path"])
        assert len(imp["files"]) == 2 and Path(imp["files"][1]).name == "36 Kick 2.wav", imp
        assert [x["name"] for x in lib.search("kick")][:1] == ["36 Kick"]
        b = lib.new_bin("Favourites")["id"]
        sub = lib.new_bin("Snares", b)["id"]
        lib.bin_add(sub, [kit["files"][2]["path"], "/etc/passwd"])
        assert lib.bin_listing(b)["bins"][0]["id"] == sub and len(lib.bin_listing(sub)["files"]) == 1
        lib.move_bin(b, sub)
        assert lib.bins()[0]["parent"] is None, "a bin can't go into its own"
        lib.delete_bin(b)
        assert lib.bins() == [], "with the bins in it"
        frames, sr, ch, peaks = Engine.wav_info(kick)
        assert sr == 48000 and ch == 1 and frames > 10000 and len(peaks) > 10

        # into the song
        a, bb, _ = connect_pair()
        da, db = Daw(a, root / "A", "alice"), Daw(bb, root / "B", "bob")
        try:
            wait_for(lambda: da.p.pid == db.p.pid and da.view()["shared"] and db.view()["shared"], what="one shared song")
            res = da.sample_clip(kick, None, 4.0)
            assert res["ok"], res
            key = next(c["file"] for c in da.view()["clips"])
            assert key.startswith("s-") and da.view()["files"][key]["name"] == "36 Kick"
            wait_for(lambda: key in db.loaded, what="the sample on bob's side", timeout=15)
            clip = db.view()["clips"][0]
            assert clip["start"] == 4.0 and abs(clip["len"] - frames / sr) < 1e-6
            assert da.import_sample(kick) == key, "the same sound twice is one file"
            snare = kit["files"][2]["path"]
            ch = da.sampler_channel(snare)["track"]
            wait_for(lambda: ch in db.slots and (bb.mixer()["tracks"][db.slots[ch]]["inst"] or {}).get("pads", [None])[0],
                     what="bob's sampler gets its sample", timeout=15)
            got = bb.mixer()["tracks"][db.slots[ch]]["inst"]
            assert got["kind"] == "sampler" and Path(got["pads"][0]).parent == db.p.audio_dir, got
            assert db.view()["tracks"][-1]["inst"]["pads"][0].startswith("file:"), "the song names the file, not a path"
            # a drum channel's pad
            da.edit([["track", "dk", {"name": "Drums", "order": 9.0, "owner": "alice", "inst": {"kind": "drums", "level": 0.8}}]])
            wait_for(lambda: "dk" in da.slots and "dk" in db.slots, what="the drum channel")
            assert da.sample_pad("dk", 2, kit["files"][5]["path"])["ok"]
            # (the instrument may not be there yet for a moment: wait for it too)
            wait_for(lambda: ((bb.mixer()["tracks"][db.slots["dk"]]["inst"] or {}).get("pads") or [None] * 3)[2], what="bob's pad", timeout=15)
            # and it plays: bob's sampler, a note on its root
            slot = db.slots[ch]
            bb.note(f"T{slot}", True, 60, 120)
            out = []
            for _ in range(20):
                out += bb.process([0.0] * N)
            assert max(abs(v) for v in out) > 0.1, "the sampler plays its sample"
            print("library: ok  (built-in sounds, folders, import, search, nested bins; a clip, a sampler channel and a pad "
                  "reach the partner and play)")
        finally:
            da.close()
            db.close()
            a.close()
            bb.close()
    finally:
        shutil.rmtree(root, ignore_errors=True)


def hits_in(out, gap=12000):
    """Sample positions where something starts sounding (after `gap` samples of nothing new)."""
    env = np.abs(np.array(out))
    hits = []
    for k in np.nonzero(env > 0.05)[0]:
        if not hits or k - hits[-1] > gap:
            hits.append(int(k))
    return hits


def wave_peak(path):
    import wave
    with wave.open(str(path)) as w:
        raw = np.frombuffer(w.readframes(w.getnframes()), dtype=np.uint8).reshape(-1, 3)
    v = (raw[:, 0].astype(np.int32) | raw[:, 1].astype(np.int32) << 8 | raw[:, 2].astype(np.int32) << 16)
    v = np.where(v >= 1 << 23, v - (1 << 24), v)
    return float(np.max(np.abs(v))) / (1 << 23)


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
    # the acks cross the same 30%-loss link: give the retransmits time to get one through
    t1 = time.monotonic()
    while a.signals_pending and time.monotonic() - t1 < 10:
        time.sleep(0.1)
        got += b.drain_signals()
    assert sorted(got) == sorted(msgs), "a retransmit was delivered twice"
    assert a.signals_pending == 0, "messages never acked"
    a.close()
    b.close()
    print(f"signals: ok  (3 messages incl. 5 KB over a 30%-loss link in {took:.1f} s)")


def test_effects_cpu():
    """Worst realistic load: 4 inputs, each with every built-in effect (a full
    rack: an empty NAM slot does nothing anyway), monitored, all in stereo."""
    e = Engine()
    every = [fx["name"] for fx in Engine.effects() if fx["name"] not in ("vst", "nam")]  # a VST slot needs a plugin
    assert len(every) == 22, every
    kinds = ["comp", "drive", "chorus", "tremolo", "delay", "reverb", "eq", "limiter"]
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
    print(f"effects: ok  (4 strips x 8 effects: {us:.1f} us per block of {N / SR * 1e6:.0f} us)")
    assert us < 1500, us
    assert set(Engine.presets()) == {"Clean", "Dreamy", "Crunch", "Vocal", "Surf"}
    # Duet's own pro plugins, set heavy: all 8 EQ bands on and dynamic, 8x oversampling
    pro = ["eqpro", "multi", "clipper", "saturate", "shaper", "comp", "limiter", "eq"]
    for i in range(4):
        t = f"L{i}"
        e.rack_load(t, [{"kind": k, "on": True} for k in pro])
        for b in range(1, 9):
            e.rack_set(t, 0, f"b{b}on", 1)
            e.rack_set(t, 0, f"b{b}dyn", 0.3)
        e.rack_set(t, 2, "os", 1.0)
        e.rack_set(t, 3, "os", 1.0)
    for _ in range(600):
        out = e.process(x, 4)
    assert np.all(np.isfinite(out))
    us2 = e.state()["cpu_us"]
    print(f"effects: ok  (4 strips x 8 pro plugins, 8x oversampling: {us2:.1f} us per block of {N / SR * 1e6:.0f} us)")
    assert us2 < 3000, us2
    # and the rest of them, with a hall reverb and a synced delay
    more = ["deess", "transient", "width", "gate", "filter", "lofi", "doubler", "reverb"]
    for i in range(4):
        t = f"L{i}"
        e.rack_load(t, [{"kind": k, "on": True} for k in more])
        e.rack_set(t, 7, "type", 2 / 3)
    for _ in range(600):
        out = e.process(x, 4)
    assert np.all(np.isfinite(out))
    us3 = e.state()["cpu_us"]
    print(f"effects: ok  (4 strips x 8 more plugins incl. a hall: {us3:.1f} us per block of {N / SR * 1e6:.0f} us)")
    assert us3 < 3000, us3
    # live pitch correction on every input (it delays them by its stretcher; that's reported)
    for i in range(4):
        e.rack_load(f"L{i}", [{"kind": "autotune", "on": True}, {"kind": "deess", "on": True}])
    for _ in range(600):
        out = e.process(x, 4)
    assert np.all(np.isfinite(out))
    us4 = e.state()["cpu_us"]
    print(f"effects: ok  (4 strips with live pitch correction: {us4:.1f} us per block of {N / SR * 1e6:.0f} us)")
    assert us4 < 2500, us4
    for i in range(4):
        e.rack_load(f"L{i}", [{"kind": k, "on": True} for k in pro])
        for b in range(1, 9):
            e.rack_set(f"L{i}", 0, f"b{b}on", 1)
    for _ in range(50):
        e.process(x, 4)
    # the EQ Pro's window: spectra before and after, and where each band is now
    sc = e.rack_scope("L0", 0)
    assert len(sc) == 136 and max(sc[:64]) > -60, sc[:8]
    # 8x oversampling makes a slot late; the strip reports it
    assert e.mixer()["local"][0]["rack"][2]["kind"] == "clipper"
    e.rack_preset("L0", "Crunch")
    assert [s["kind"] for s in e.mixer()["local"][0]["rack"]] == ["comp", "drive", "reverb"]
    e.rack_preset("L0", "Vocal")
    assert [s["kind"] for s in e.mixer()["local"][0]["rack"]] == ["eq", "comp", "reverb"]
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


def test_plugin_path(name="test_gain"):
    """A test plugin from engine/examples (test_gain: the vst3 crate's gain
    example; test_synth: a sine instrument), packaged as a .vst3. Built with:
    cd engine && cargo build --release --example test_gain --example test_synth"""
    import shutil
    import tempfile
    from pathlib import Path
    ex = Path(__file__).resolve().parent / "engine" / "target" / "release" / "examples"
    lib = next((p for p in (ex / f"{name}.dll", ex / f"lib{name}.dylib", ex / f"lib{name}.so") if p.exists()), None)
    if lib is None:
        return None
    out = Path(tempfile.mkdtemp()) / f"{name}.vst3"
    if lib.suffix == ".dll":
        shutil.copy(lib, out)                       # a single-file .vst3 is fine on Windows
    elif lib.suffix == ".so":
        (out / "Contents" / "x86_64-linux").mkdir(parents=True)
        shutil.copy(lib, out / "Contents" / "x86_64-linux" / f"{name}.so")
    else:
        (out / "Contents" / "MacOS").mkdir(parents=True)
        shutil.copy(lib, out / "Contents" / "MacOS" / name)
        (out / "Contents" / "Info.plist").write_text(
            '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>'
            f"<key>CFBundleExecutable</key><string>{name}</string>"
            f"<key>CFBundleIdentifier</key><string>app.duet.{name.replace('_', '')}</string>"
            "<key>CFBundlePackageType</key><string>BNDL</string></dict></plist>")
    return str(out)


def test_au():
    """Audio Units (Mac only): Apple's own AUDelay and DLS synth are on every Mac. Listed, loaded
    into a rack (an echo comes out), played as an instrument, saved and restored."""
    if sys.platform != "darwin":
        print("au: skipped (Audio Units are a Mac thing)")
        return
    aus = Engine.au_list()
    paths = {a["path"]: a for a in aus}
    delay = "au:61756678:64656c79:6170706c"   # aufx dely appl
    dls = "au:61756d75:646c7320:6170706c"     # aumu 'dls ' appl
    assert delay in paths and dls in paths and paths[dls]["instrument"], [a["name"] for a in aus][:20]
    x = [0.0] * N
    e = Engine()
    e.strip_set("L0", "mon", 1)
    e.rack_add_vst("L0", delay, "au", paths[delay]["name"])
    slot = e.mixer()["local"][0]["rack"][0]
    assert slot["kind"] == "vst" and slot["vst"]["name"], slot
    imp = [0.0] * N
    imp[0] = 0.9
    out = list(e.process(imp))
    for _ in range(400):  # AUDelay: 1 s by default
        out += e.process(x)
    out = np.abs(np.array(out))
    late = np.nonzero(out[2000:] > 0.01)[0]
    assert len(late), "an echo comes out after the dry click"
    snap = e.mixer(True)["local"][0]["rack"]
    assert str(snap[0]["vst"].get("state") or "").startswith("au:"), snap
    e2 = Engine()
    e2.strip_set("L0", "mon", 1)
    e2.rack_load("L0", snap)
    assert e2.mixer()["local"][0]["rack"][0]["vst"]["path"] == delay
    # its parameters, and automation: the wet/dry mix all the way dry on the master, so no echo
    ps = e.plugin_params("L0", 0)
    wet = [p for p in ps if "wet" in p["name"].lower()]
    assert len(ps) >= 3 and wet, [p["name"] for p in ps]
    m = Engine()
    m.strip_set("L0", "mon", 1)
    m.rack_add_vst("M", delay, "au", paths[delay]["name"])
    assert m.daw_automation([("M", f"fx:0:p{wet[0]['id']}", [(0.0, 0.0), (60.0, 0.0)])]) == []
    m.process(x)
    out = list(m.process(imp))
    for _ in range(400):
        out += m.process(x)
    assert np.max(np.abs(np.array(out[2000:]))) < 0.01, "automated fully dry: no echo"
    m.close()
    # the DLS synth as an instrument
    v = Engine()
    v.strip_set("L0", "mon", 1)
    v.inst_set("L0", "vst", (dls, "au", paths[dls]["name"]))
    v.note("L0", True, 69, 110)
    played = []
    for _ in range(80):
        played += v.process(x)
    assert max(abs(s) for s in played) > 0.01, "the AU synth plays a note"
    for z in (e, e2, v):
        z.close()
    print(f"au: ok  ({len(aus)} Audio Units listed; AUDelay echoes, its settings save and restore, its {len(ps)} parameters automate; the DLS synth plays)")


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
    # its parameters, and automation moving one: the gain to half on the master
    ps = e.plugin_params("L0", 0)
    assert [(p["id"], p["name"]) for p in ps] == [(0, "Gain")] and abs(ps[0]["default"] - 1.0) < 1e-6, ps
    assert e.plugin_touched("L0", 0) is None
    m = Engine()
    m.strip_set("L0", "mon", 1)
    m.rack_add_vst("M", path, classes[0]["cid"], classes[0]["name"])
    assert m.daw_automation([("M", "fx:0:p0", [(0.0, 0.5), (10.0, 0.5)])]) == []
    for _ in range(5):
        auto_out = m.process(x)
    assert np.allclose(auto_out, np.multiply(dry_out, 0.5), atol=1e-5), np.max(np.abs(np.subtract(auto_out, np.multiply(dry_out, 0.5))))
    assert m.daw_automation([("M", "fx:0:p7x", [(0.0, 0.5)])]) == ["M fx:0:p7x"]
    m.close()
    # removing it tears the instance down (off the audio thread)
    e.rack_remove("L0", 0)
    e.process(x)
    assert e.mixer()["local"][0]["rack"] == []
    for z in (e, e2, dry):
        z.close()
    print("vst: ok  (scan, load with stereo fallback, audio, state save/restore, parameters, automation, teardown)")


def test_stun():
    e = Engine()
    e.start()  # the receive thread picks up the STUN reply
    code = e.discover()
    print(f"stun: public {e.public_addr}, code {code}")
    assert e.public_addr, "no STUN server answered"
    e.close()


def test_channel_blocks():
    """A pattern block on a channel's row plays that channel's notes. Writing a second channel into a
    pattern that's already in the song gives that channel its own block in the same place (one undo
    with the notes; a block you deleted stays deleted); old songs get those blocks when opened."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw

    e = Engine()
    e.start()
    root = Path(tempfile.mkdtemp(prefix="duet-blocks-"))
    d = Daw(e, root / "songs", "me")
    try:
        d.edit([["track", "syn", {"name": "Synth", "order": 5.0, "owner": "me", "inst": {"kind": "synth"}}],
                ["track", "drm", {"name": "Drums", "order": 6.0, "owner": "me", "inst": {"kind": "drums"}}],
                ["pattern", "p1", {"name": "Verse", "len": 4.0}],
                ["note", "s1", {"pattern": "p1", "track": "syn", "start": 0.0, "len": 1.0, "key": 60, "vel": 100}],
                ["clip", "c1", {"track": "syn", "pattern": "p1", "start": 8.0, "blen": 4.0, "boff": 0.0}]])
        assert [c["track"] for c in d.view()["clips"]] == ["syn"]
        # the drum kit gets notes in the same pattern: its own block, in the same place
        d.edit([["note", "d1", {"pattern": "p1", "track": "drm", "start": 0.5, "len": 0.25, "key": 36, "vel": 100}]])
        blocks = {c["track"]: c for c in d.view()["clips"]}
        assert set(blocks) == {"syn", "drm"} and blocks["drm"]["start"] == 8.0 and blocks["drm"]["blen"] == 4.0, blocks
        # each block plays its own channel: one note per channel, not two on each
        notes = d._notes(d._tm().b2s)
        assert len(notes[d.slots["syn"]]) == 1 and len(notes[d.slots["drm"]]) == 1, notes
        assert notes[d.slots["drm"]][0][2] == 36 and notes[d.slots["syn"]][0][2] == 60
        # one undo takes the note and its block back
        d.undo()
        assert [c["track"] for c in d.view()["clips"]] == ["syn"] and len(d.view()["notes"]) == 1
        d.redo()
        mirror = next(c for c in d.view()["clips"] if c["track"] == "drm")
        # deleted on purpose: it stays deleted when more drum notes come
        d.edit([["clip", mirror["id"], {"del": True}]])
        d.edit([["note", "d2", {"pattern": "p1", "track": "drm", "start": 1.5, "len": 0.25, "key": 38, "vel": 100}]])
        assert [c["track"] for c in d.view()["clips"]] == ["syn"]
        # an old song (one block playing both channels): opened, the drums get their block
        del d.p.objs["clip"][mirror["id"]]  # (as if it had never been made)
        d.p.stamps["clip"].pop(mirror["id"], None)
        d._upgrade_blocks()
        assert {c["track"] for c in d.view()["clips"]} == {"syn", "drm"}
        # looping: a block repeats one round (the base length), a "once" block plays the pattern through once
        d.edit([["pattern", "p1", {"len": 16.0, "base": 4.0}], ["clip", "c1", {"blen": 16.0}]])
        starts = sorted(round(n[0], 3) for n in d._notes(d._tm().b2s)[d.slots["syn"]])
        assert starts == [4.0, 6.0, 8.0, 10.0], starts  # (beat 8, 12, 16, 20 at 120 bpm)
        d.edit([["clip", "c1", {"once": True}]])
        starts = sorted(round(n[0], 3) for n in d._notes(d._tm().b2s)[d.slots["syn"]])
        assert starts == [4.0], starts
        print("channel blocks: each channel plays its own notes from its own row; a new channel's notes get a block there; loop / once")
    finally:
        d.close()
        e.close()
        shutil.rmtree(root, ignore_errors=True)


def test_clip_gain():
    """A pattern block's gain line and fades act on what its channel plays (the "cgain" curve)."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw

    e = Engine()
    e.start()
    root = Path(tempfile.mkdtemp(prefix="duet-cgain-"))
    d = Daw(e, root / "songs", "me")
    x = [0.0] * N
    try:
        d.set_local("click", False)
        d.edit([["meta", "meta", {"bpm": 120.0}],
                ["track", "syn", {"name": "Synth", "order": 5.0, "owner": "me", "vol": 1.0, "inst": {"kind": "synth"}}],
                ["pattern", "p1", {"name": "Pad", "len": 8.0}],
                ["note", "n1", {"pattern": "p1", "track": "syn", "start": 0.0, "len": 8.0, "key": 57, "vel": 100}],
                ["clip", "c1", {"track": "syn", "pattern": "p1", "start": 0.0, "blen": 8.0, "boff": 0.0}]])

        def level(t0, t1):
            d.play(0.0)
            out = []
            for _ in range(int(t1 * SR / N)):
                out += e.process(x)
            d.stop()
            y = np.array(out[int(t0 * SR):])
            return float(np.sqrt(np.mean(y * y)))
        full = level(1.0, 2.0)
        d.edit([["clip", "c1", {"gain": 0.5}]])
        half = level(1.0, 2.0)
        assert 0.4 < half / full < 0.6, (full, half)
        # a 2 s fade in: quieter at 0.5 s than at 1.5 s (full gain again)
        d.edit([["clip", "c1", {"gain": 1.0, "fade_in": 2.0}]])
        early, late = level(0.3, 0.7), level(1.4, 1.8)
        assert early < 0.5 * late, (early, late)
        # a muted note (the piano roll's mute tool) is silent, and comes back
        d.edit([["clip", "c1", {"fade_in": 0.0}], ["note", "n1", {"mute": True}]])
        assert level(1.0, 2.0) < 0.05 * full
        d.edit([["note", "n1", {"mute": False}]])
        assert level(1.0, 2.0) > 0.8 * full
        # a controller lane: volume (CC7) at half from beat 2 is about a quarter of the level there
        # (and the lane's value holds before its first point); then a ramp of the pitch wheel
        d.edit([["ctl", "k7", {"pattern": "p1", "track": "syn", "cc": 7, "points": [[2.0, 1.0], [2.01, 0.5]]}]])
        before, after = level(0.5, 0.9), level(1.4, 1.9)
        assert 0.8 < before / full < 1.2 and 0.15 < after / full < 0.35, (full, before, after)
        d.edit([["ctl", "k7", {"del": True}], ["ctl", "kb", {"pattern": "p1", "track": "syn", "cc": 128, "points": [[0.0, 0.5], [4.0, 1.0]]}]])
        moves = d._ctls(lambda b: b / 2)[d.slots["syn"]]
        assert moves[0] == (0.0, 128, 0.5) and abs(moves[-1][2] - 1.0) < 1e-3 and all(m[1] == 128 for m in moves), moves[:3]
        assert all(b[2] >= a[2] for a, b in zip(moves, moves[1:])) and 100 < len(moves) < 200, len(moves)
        d.edit([["ctl", "kb", {"del": True}]])
        # a track whose effects delay it (the limiter looks ahead) reads ahead of the song: the note
        # on the very first beat must still play when play starts there (it used to be skipped, so
        # the first bar went missing on such tracks until the loop came round)
        d.edit([["note", "n1", {"len": 0.5}],
                ["track", "syn", {"rack": [{"kind": "limiter", "on": True, "gain": 1.0, "ceiling": 0.0, "release": 0.4}]}]])
        first = level(0.02, 0.2)
        assert first > 0.3 * full, (first, full)
        d.edit([["note", "n1", {"len": 8.0}], ["track", "syn", {"rack": []}]])
        # an empty track (no input) can be armed, but there's nothing to record from it: Record says so
        d.edit([["track", "empty", {"name": "Track 2", "order": 6.0, "owner": "me", "src": -1, "arm": True, "vol": 0.8}]])
        r = d.record(0.0)
        assert "no input" in (r.get("error") or ""), r
        d.edit([["track", "empty", {"del": True}]])
        # the pattern on its own, from beat 4 of it (the piano roll's ruler)
        d.play_pattern("p1", 4.0)
        e.process(x)
        pos = e.state()["daw_pos"]  # seconds: beat 4 at 120 BPM is 2 s
        assert d.pattern_mode == "p1" and 1.95 < pos < 2.2, pos
        d.stop()
        print(f"clip gain: ok  (a pattern block at half gain: {half / full:.2f} of the level; a fade in rises; a muted note is silent; "
              f"CC7 at half: {after / full:.2f})")
    finally:
        d.close()
        e.close()
        shutil.rmtree(root, ignore_errors=True)


def test_midi_files():
    """A MIDI file in and out: our writer read back exactly; a file imported as new channels (a drum
    kit for channel 10) and a pattern in the playlist, the song exported again with the same notes;
    a file into an existing pattern's channel."""
    import shutil
    import tempfile
    from pathlib import Path

    import midifile
    from daw import Daw

    e = Engine()
    e.start()
    root = Path(tempfile.mkdtemp(prefix="duet-midi-"))
    d = Daw(e, root / "songs", "me")
    try:
        lead = [(0.0, 1.0, 60, 100), (1.0, 0.5, 64, 90), (1.5, 0.5, 67, 80), (2.0, 2.0, 72, 110)]
        kick = [(float(b), 0.25, 36, 120) for b in range(8)]
        f = root / "in.mid"
        midifile.write(f, [{"name": "Lead", "channel": 0, "notes": lead, "bend": [(1.0, 0.5)], "cc": [(0.5, 1, 64)]},
                           {"name": "Beat", "channel": 9, "notes": kick}], bpm=93.0, beats=4)
        m = midifile.read(f)
        assert abs(m["bpm"] - 93.0) < 0.01 and m["beats"] == 4, m
        got = {t["name"]: t for t in m["tracks"]}
        assert [tuple(round(x, 4) for x in n) for n in got["Lead"]["notes"]] == lead and len(got["Beat"]["notes"]) == 8
        assert got["Lead"]["cc"] == [(0.5, 1, 64)] and abs(got["Lead"]["bend"][0][1] - 0.5) < 0.001
        r = d.import_midi(str(f))
        assert r.get("ok") and r["notes"] == 12 and sorted(r["channels"]) == ["Beat", "Lead"], r
        v = d.view()
        kinds = {t["name"]: (t.get("inst") or {}).get("kind") for t in v["tracks"]}
        assert kinds["Beat"] == "drums" and kinds["Lead"] == "synth", kinds
        assert abs(v["meta"]["bpm"] - 93.0) < 0.01, v["meta"]["bpm"]
        assert len([c for c in v["clips"] if c.get("pattern") == r["pattern"]]) == 2
        lanes = {c["cc"]: c["points"] for c in v["ctls"]}
        assert sorted(lanes) == [1, 128] and lanes[1] == [[0.5, round(64 / 127, 4)]] and lanes[128] == [[1.0, 0.75]], lanes
        out = root / "out.mid"
        x = d.export_midi(str(out))
        assert x.get("ok") and x["notes"] == 12, x
        back = {t["name"]: sorted(n[:3] for n in t["notes"]) for t in midifile.read(out)["tracks"]}
        assert back["Lead"] == sorted(n[:3] for n in lead) and len(back["Beat"]) == 8, back
        lead_back = [t for t in midifile.read(out)["tracks"] if t["name"] == "Lead"][0]
        assert lead_back["cc"] == [(0.0, 1, 64)] and abs(lead_back["bend"][0][1] - 0.5) < 0.001, lead_back
        # into an existing pattern's channel (the piano roll)
        lid = [t["id"] for t in v["tracks"] if t["name"] == "Lead"][0]
        before = len([n for n in d.view()["notes"] if n["track"] == lid])
        assert d.import_midi(str(f), r["pattern"], lid).get("ok")
        assert len([n for n in d.view()["notes"] if n["track"] == lid]) == before + 12
        assert len([c for c in d.view()["ctls"] if c["track"] == lid]) == 4, "the lanes too"
        # MIDI-learn: the engine keeps where each of the keyboard's controllers is, and the last one moved
        count0 = e.midi_controllers()[0]
        e.midi_feed([0xB3, 21, 100])
        e.midi_feed([0xE0, 0, 0])
        count, last, vals = e.midi_controllers()
        assert count == count0 + 2 and last == 128 and abs(vals[21] - 100 / 127) < 1e-6 and vals[128] == 0.0 and vals[22] == -1.0
        print("midi files: ok  (written and read back exactly; imported as 2 channels and a pattern with CC and pitch-bend lanes, "
              "exported again; into a channel)")
    finally:
        d.close()
        e.close()
        shutil.rmtree(root, ignore_errors=True)


def test_takes():
    """Loop recording makes a take each time round (the last whole one plays); comping picks a take
    for a stretch; punch keeps only what's between the in and out points, stops by itself after the
    out point, and over a clip makes a take folder with it."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw

    e = Engine()
    e.start()
    root = Path(tempfile.mkdtemp(prefix="duet-takes-"))
    d = Daw(e, root / "songs", "me")
    zero = [0.0] * N

    def tone(amp, blocks):
        for k in range(blocks):
            e.process([amp * math.sin(2 * math.pi * 220 * (k * N + i) / SR) for i in range(N)])
            if k % 50 == 49:
                time.sleep(0.01)  # no faster than the take writer drains (a sound card never is)

    def rms_at(t0, t1):
        """The song's level from t0 to t1 seconds, played."""
        e.daw_play(t0)
        out = []
        for _ in range(int((t1 - t0) * SR / N)):
            out += e.process(zero)
        e.daw_stop()
        return float(np.sqrt(np.mean(np.square(out))))

    try:
        tid = d.view()["tracks"][0]["id"]
        d.edit([["meta", "meta", {"bpm": 120.0, "loop": True, "loop_start": 0.0, "loop_end": 4.0}],
                ["track", tid, {"arm": True}]])
        d.set_local("countin", False)
        d.set_local("click", False)
        d.record(0.0)
        per = int(2.0 * SR / N)  # one time round the 2 s loop
        for k, amp in enumerate([0.1, 0.2, 0.4]):
            tone(amp, per)
        tone(0.3, per // 3)  # a fourth time, cut short
        d.stop()
        assert not any("skipped" in n for n in d.notices), d.notices
        v = d.view()
        takes = sorted(v["takes"], key=lambda c: c["take"])
        assert [c["take"] for c in takes] == [1, 2, 3, 4] and len({c["group"] for c in takes}) == 1, takes
        assert all(abs(c["len"] - 2.0) < 0.02 for c in takes[:3]) and takes[3]["len"] < 1.0, [c["len"] for c in takes]
        comp = [c for c in v["clips"] if c.get("group")]
        assert len(comp) == 1 and comp[0]["take"] == 3 and comp[0]["start"] == 0.0, comp
        group = comp[0]["group"]
        d.edit([["track", tid, {"arm": False}]])
        d.set_local("countin", False)
        d.edit([["meta", "meta", {"loop": False}]])
        hi = rms_at(0.2, 1.8)
        # comping: the first half from take 1
        d.comp(group, 1, 0.0, 2.0)
        v = d.view()
        comp = sorted((c for c in v["clips"] if c.get("group") == group), key=lambda c: c["start"])
        assert [c["take"] for c in comp] == [1, 3] and comp[1]["start"] == 2.0, comp
        a, b = rms_at(0.2, 0.9), rms_at(1.1, 1.8)
        assert 0.15 < a / b < 0.4, (a, b, hi)  # take 1 (0.1) against take 3 (0.4)
        assert abs(b - hi) / hi < 0.1, (b, hi)
        d.undo()
        assert len([c for c in d.view()["clips"] if c.get("group")]) == 1, "a comp is one undo"

        # punch: a take from beat 8 to 16, then a punch in 10..12 over it
        d.edit([["track", tid, {"arm": True}]])
        d.record(8.0)
        tone(0.1, int(4.0 * SR / N))
        d.stop()
        plain = [c for c in d.view()["clips"] if not c.get("group")]
        assert len(plain) == 1 and plain[0]["start"] == 8.0, plain
        d.edit([["meta", "meta", {"loop_start": 10.0, "loop_end": 12.0}]])
        d.set_local("punch", True)
        assert d.record(9.0) == {"ok": True}
        stopped = None
        for k in range(int(4.0 * SR / N)):
            tone(0.4, 1)
            if k % 20 == 0:
                d.status(e.state())
                if not d.rec:
                    stopped = k * N / SR
                    break
        assert stopped is not None and 1.5 < stopped < 2.2, f"the punch stops after the out point ({stopped})"
        v = d.view()
        comp = sorted((c for c in v["clips"] if c["start"] >= 8.0), key=lambda c: c["start"])
        assert [(c["start"], c["take"]) for c in comp] == [(8.0, 1), (10.0, 2), (12.0, 1)], comp
        assert abs(comp[1]["len"] - 1.0) < 0.01, comp[1]
        g2 = comp[0]["group"]
        assert {c["take"] for c in v["takes"] if c["group"] == g2} == {1, 2}
        d.set_local("punch", False)
        d.take_op(g2, "flatten")
        v = d.view()
        assert not [c for c in v["takes"] if c["group"] == g2] and all(not c.get("group") for c in v["clips"] if c["start"] >= 8)
        print("takes: loop recording a take a time round, comping, punch in/out over a clip")
    finally:
        d.close()
        e.close()
        shutil.rmtree(root, ignore_errors=True)


def test_tempo_map():
    """A tempo change and a time signature change: notes, the click (its accents on the new bars), the
    playhead and recording follow them; deleting and duplicating a section moves everything after it."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw, TempoMap

    tm = TempoMap({"bpm": 120, "beats": 4, "tempo": [[4, 60], [-1, 99], [6, "x"]], "meters": [[8, 3]]})
    assert tm.b2s(4) == 2.0 and tm.b2s(6) == 4.0 and abs(tm.s2b(4.0) - 6) < 1e-9 and tm.s2b(1.0) == 2.0
    assert tm.bpm_at(3.9) == 120 and tm.bpm_at(4) == 60 and tm.meter_at(9) == (8.0, 3) and tm.bar_secs(0) == 2.0
    assert tm.click_map() == [(0.0, 120.0, 0.0, 4, 0.0), (2.0, 60.0, 4.0, 4, 0.0), (6.0, 60.0, 8.0, 3, 8.0)]
    assert TempoMap({"bpm": 100, "beats": 4}).click_map() == [], "a steady song: the engine's own click"

    e = Engine()
    e.start()
    root = Path(tempfile.mkdtemp(prefix="duet-tempo-"))
    d = Daw(e, root / "songs", "me")
    x = [0.0] * N
    try:
        d.edit([["meta", "meta", {"bpm": 120.0, "beats": 4, "tempo": [[4.0, 60.0]], "meters": [[8.0, 3]]}],
                ["track", "drums", {"name": "Drums", "order": 5.0, "owner": "me", "vol": 1.0, "pan": 0.0,
                                    "inst": {"kind": "drums", "level": 0.8, "tune": 0.5, "decay": 1.0}}],
                ["pattern", "p1", {"name": "Beat", "len": 1.0}],
                ["note", "n0", {"pattern": "p1", "track": "drums", "start": 0.0, "len": 0.25, "key": 42, "vel": 120}],
                ["clip", "pc", {"track": "drums", "pattern": "p1", "start": 0.0, "blen": 7.0, "boff": 0.0}]])
        d.set_local("click", False)
        d.play(0.0)
        out = []
        for _ in range(int(5.5 * SR / N)):
            out += e.process(x)
        st = d.status(e.state())
        d.stop()
        want = [round(tm.b2s(k) * SR) for k in range(7)]  # 0, .5, 1, 1.5, then a second a beat
        got = hits_in(out)
        assert len(got) == 7 and all(abs(h - w) < 4 for h, w in zip(got, want)), (got, want)
        assert abs(st["pos"] - tm.s2b(len(out) / SR)) < 0.1, st["pos"]

        # the click: beats where the map says, accents on each bar's first beat (0, 4, then 3/4 from 8)
        d.edit([["clip", "pc", {"del": True}]])
        d.set_local("click", True)
        d.play(0.0)
        out = []
        for _ in range(int(12.2 * SR / N)):
            out += e.process(x)
        d.stop()
        d.set_local("click", False)
        out = np.array(out)
        got = hits_in(out, gap=6000)
        want = [round(tm.b2s(k) * SR) for k in range(15)]  # 12.2 s: up to beat 14
        assert len(got) == len(want) and all(abs(h - w) < 4 for h, w in zip(got, want)), (got, want)
        peaks = [float(np.abs(out[h:h + 2000]).max()) for h in got]
        loud = [k for k, p in enumerate(peaks) if p > 0.85 * max(peaks)]
        assert loud == [0, 4, 8, 11, 14], (loud, peaks)

        # delete beats 4..8 (the slow 4/4 bar): what came after moves 4 beats earlier, tempo joins up
        d.edit([["marker", "m1", {"beat": 4.0, "name": "Verse", "section": True}],
                ["marker", "m2", {"beat": 8.0, "name": "Chorus", "section": True}],
                ["clip", "c9", {"track": "drums", "pattern": "p1", "start": 9.0, "blen": 1.0, "boff": 0.0}],
                ["clip", "c5", {"track": "drums", "pattern": "p1", "start": 5.0, "blen": 1.0, "boff": 0.0}],
                ["track", "drums", {"points": [[2.0, 0.1], [5.0, 0.5], [10.0, 0.9]]}]])
        v = d.time_op("delete", 4.0, 8.0)
        clips = {c["id"]: c for c in v["clips"]}
        assert "c5" not in clips and clips["c9"]["start"] == 5.0, clips
        assert [(m["name"], m["beat"]) for m in v["markers"]] == [("Chorus", 4.0)], v["markers"]
        assert v["meta"]["tempo"] == [[4.0, 60.0]] and v["meta"]["meters"] == [[4.0, 3]], v["meta"]
        assert next(t for t in v["tracks"] if t["id"] == "drums")["points"] == [[2.0, 0.1], [6.0, 0.9]]
        d.undo()
        v = d.view()
        assert {c["id"] for c in v["clips"]} == {"c5", "c9"} and len(v["markers"]) == 2, "one undo brings it back"

        # duplicate the verse: a copy right after, the chorus later; the copy starts at the verse's tempo
        v = d.time_op("duplicate", 4.0, 8.0)
        starts = sorted(c["start"] for c in v["clips"])
        assert starts == [5.0, 9.0, 13.0], starts
        assert [(m["name"], m["beat"]) for m in v["markers"]] == [("Verse", 4.0), ("Verse", 8.0), ("Chorus", 12.0)]
        assert v["meta"]["tempo"] == [[4.0, 60.0]] and v["meta"]["meters"] == [[12.0, 3]], v["meta"]
        print("tempo map: notes, click and accents follow the tempo and time signature changes; sections move")
    finally:
        d.close()
        e.close()
        shutil.rmtree(root, ignore_errors=True)


def test_versions():
    """Versions (the song as it was, restored as one undoable edit), a damaged song file (the save before
    stands in), a crash (said on the next start), and a song file (.duet: the song and its audio) that opens
    as a song of its own."""
    import shutil
    import tempfile
    from pathlib import Path

    from daw import Daw

    e = Engine()
    e.start()
    root = Path(tempfile.mkdtemp(prefix="duet-ver-"))
    d = Daw(e, root / "songs", "me")
    try:
        tid = d.view()["tracks"][0]["id"]
        d.edit([["track", tid, {"arm": True}]])
        d.set_local("countin", False)
        d.record(0.0)
        x = [0.3 * math.sin(2 * math.pi * 220 * i / SR) for i in range(N)]
        for _ in range(375):
            e.process(x)
        d.stop()
        clip = d.view()["clips"][0]
        d.edit([["meta", "meta", {"name": "Take one"}]])
        d.p.save()
        d._snapshot()
        vid = d.versions()[0]["id"]
        # then: a new name, the clip moved, a second track; and back
        time.sleep(1.1)  # (a version a second: the names are to the second)
        d.edit([["meta", "meta", {"name": "Take two"}], ["clip", clip["id"], {"start": 7.5}],
                ["track", "t-new", {"name": "Bass", "order": 2.0, "owner": "me", "src": 0, "vol": 0.8, "pan": 0.0, "rack": []}]])
        assert len(d.view()["tracks"]) == 2
        r = d.restore_version(vid)
        assert r.get("ok"), r
        v = d.view()
        assert v["meta"]["name"] == "Take one" and len(v["tracks"]) == 1 and v["clips"][0]["start"] == clip["start"], v["meta"]
        assert len(d.versions()) >= 2, "the state before the restore is kept as a version too"
        d.undo()
        assert d.view()["meta"]["name"] == "Take two", "a restore is undone in one step"
        # a damaged project.json: the save before it opens instead
        d.p.save()
        d.p.save()
        folder = d.p.folder
        (folder / "project.json").write_text('{"pid": "broken', encoding="utf-8")
        from daw import Project
        p = Project.load(folder)
        assert p.recovered == "project.json.bak" and p.meta()["name"] == "Take two", (p.recovered, p.meta())
        d.p.save()
        # a song file: save it, open it (a copy: the id is here already), the audio comes along
        bundle = str(root / "take.duet")
        r = d.save_bundle(bundle)
        assert r.get("ok") and r["files"] == 1, r
        r = d.open_bundle(bundle)
        assert r.get("ok"), r
        v2 = d.view()
        assert v2["meta"]["name"] == "Take two" and d.p.pid != p.pid and len(v2["clips"]) == 1
        assert (d.p.audio_dir / v2["clips"][0]["file"]).is_file(), "the audio came with it"
        d2_pid = d.p.pid
    finally:
        d._running = False  # a crash: no close(), the lock stays
        e.close()
    # the next start says so
    e = Engine()
    e.start()
    d = Daw(e, root / "songs", "me")
    try:
        notes = d.view().get("notices") or d.notices
        assert any("didn't close properly" in n for n in notes), notes
        assert d.p.pid == d2_pid
        print(f"versions: ok  (restored as one edit and undone, a damaged song file opened from its .bak, "
              f"a crash said on the next start, a .duet song file with its audio)")
    finally:
        d.close()
        e.close()
        shutil.rmtree(root, ignore_errors=True)


def test_updater():
    """The updater picks the newest release with this computer's download (one without it is
    skipped), never an older or the same build, downloads it whole, and a copy from source never
    updates itself."""
    import io
    import shutil
    import tempfile
    from pathlib import Path

    import updater

    def rel(run, names, draft=False):
        return {"tag_name": f"build-20261103-{run}", "draft": draft, "prerelease": False, "html_url": f"page{run}",
                "assets": [{"name": n, "browser_download_url": f"https://x/{run}/{n}", "size": 5000} for n in names]}
    mac, intel, win = "Duet-mac-apple-silicon.zip", "Duet-mac-intel.zip", "Duet-setup.exe"
    rels = [rel(112, [mac], draft=True), rel(111, [mac, win]), rel(110, [mac, intel, win]), rel(90, [intel]),
            {"tag_name": "models-htdemucs-1", "assets": [{"name": "htdemucs.onnx"}]}]
    assert updater.newest(rels, mac)["run"] == 111 and updater.newest(rels, intel)["run"] == 110
    assert updater.newest(rels, "nothing.zip") is None and updater.run_of("v1.2") is None
    root = Path(tempfile.mkdtemp(prefix="duet-upd-"))
    try:
        orig = updater.asset_name, updater.Updater.supported
        updater.asset_name = lambda: mac
        updater.Updater.supported = lambda self: None if self.current() is not None else "from source"
        u = updater.Updater({"run": 105}, root)
        r = u.check(fetch=lambda: rels)
        assert r["available"] and r["latest"] == 111 and r["size"] == 5000, r
        assert not updater.Updater({"run": 111}, root).check(fetch=lambda: rels)["available"], "the same build isn't an update"
        assert not updater.Updater(None, root).check(fetch=lambda: rels)["supported"], "from source: no updates"
        # the download, in pieces, into the updates folder; a short one is an error
        class Resp(io.BytesIO):
            headers = {"Content-Length": "5000"}
            def __enter__(self): return self
            def __exit__(self, *a): self.close()
        u._download(dict(u.found), opener=lambda req, **k: Resp(b"x" * 5000))
        st = u.status()
        assert st["state"] == "ready" and u.file.read_bytes() == b"x" * 5000 and u.file.name == f"111-{mac}", (st, u.file)
        u._download({**u.found, "run": 112}, opener=lambda req, **k: Resp(b"x" * 10))
        assert u.status()["state"] == "error" and not list(root.glob("*.part")), u.status()
        print("updater: ok  (newest release with this computer's download, not older or the same, downloaded whole)")
    finally:
        updater.asset_name, updater.Updater.supported = orig
        shutil.rmtree(root, ignore_errors=True)


def test_ui_names():
    """The page's scripts share one global scope: two top-level functions with the same name in
    different files means the later file's silently replaces the earlier one (that's how the
    playlist's ruler once ran the piano roll's click code)."""
    import re
    from pathlib import Path

    ui = Path(__file__).resolve().parent / "ui"
    order = re.findall(r'<script src="([\w.]+\.js)"', (ui / "index.html").read_text(encoding="utf-8"))
    seen, dup = {}, []
    for f in order + ["index.html"]:
        text = (ui / f).read_text(encoding="utf-8")
        for m in re.finditer(r"^(?:async\s+)?function\s+(\w+)|^(?:const|let|class)\s+(\w+)\b", text, re.M):
            name = m.group(1) or m.group(2)
            if name in seen and seen[name] != f:
                dup.append(f"{name}: {seen[name]} and {f}")
            seen.setdefault(name, f)
    assert not dup, "the same global name in two scripts: " + "; ".join(dup)
    print(f"ui names: ok  ({len(seen)} globals in {len(order) + 1} files, none twice)")


if __name__ == "__main__":
    test_ui_names()
    test_updater()
    test_effects_cpu()
    test_two_engines()
    test_auto_buffer()
    test_daw()
    test_song_edit()
    test_versions()
    test_tempo_map()
    test_takes()
    test_channel_blocks()
    test_clip_gain()
    test_midi_files()
    test_instruments()
    test_patterns()
    test_live()
    test_routing()
    test_automation()
    test_big_file()
    test_clip_edit()
    test_stems()
    test_tune()
    test_library()
    test_signals()
    test_mixer()
    test_partner_fx()
    test_metronome()
    test_nam()
    test_tone3000()
    test_vst()
    test_au()
    if "--compat" in sys.argv:   # needs git history
        test_old_build_compat()
    if "--stun" in sys.argv:
        test_stun()
    print("all good")
