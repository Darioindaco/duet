"""Self-test without a sound card, against the Rust engine (duet_engine).

The engine's own unit tests (jitter buffer, effects, codes) run with
`cargo test --release` in engine/. This checks the pieces end to end:

    python selftest.py          # add --stun to also check the public address lookup
"""
from __future__ import annotations

import math
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
        print(f"song edit: ok  (split, undo/redo incl. the take, export: {len(res['files'])} files of {secs:.2f} s "
              f"in {time.monotonic() - t0:.2f} s)")
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
    # the synth, then back to the input
    a.inst_set("L0", "synth", None, {"cutoff": 0.9})
    assert abs(a.mixer()["local"][0]["inst"]["cutoff"] - 0.9) < 1e-6
    a.note("L0", True, 60, 100)
    out = []
    for _ in range(30):
        out += a.process(x)
    spec = np.abs(np.fft.rfft(np.array(out[-2048:]) * np.hanning(2048)))
    f0 = np.argmax(spec[:200]) * SR / 2048
    assert abs(f0 - 261.6) < 25, f"middle C, not {f0:.0f} Hz"
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

        # pattern mode: just "Beat", looped, whatever else is in the song
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
            wait_for(lambda: (bb.mixer()["tracks"][db.slots["dk"]]["inst"]["pads"] or [None] * 3)[2], what="bob's pad", timeout=15)
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
    assert a.signals_pending == 0, "messages never acked"
    a.close()
    b.close()
    print(f"signals: ok  (3 messages incl. 5 KB over a 30%-loss link in {took:.1f} s)")


def test_effects_cpu():
    """Worst realistic load: 4 inputs, each with every built-in effect (a full
    rack: an empty NAM slot does nothing anyway), monitored, all in stereo."""
    e = Engine()
    kinds = [fx["name"] for fx in Engine.effects() if fx["name"] not in ("vst", "nam")]  # a VST slot needs a plugin
    assert len(kinds) == 8, kinds
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
    test_auto_buffer()
    test_daw()
    test_song_edit()
    test_instruments()
    test_patterns()
    test_live()
    test_routing()
    test_library()
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
