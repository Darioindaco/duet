"""Duet's DAW side: one project that both of you work in at the same time.

The project is a set of objects (settings, tracks, clips, audio files), each a
handful of fields. Every change is an op: new values for some fields, stamped
with a Lamport clock and who made it. Both sides apply every op from either
side and keep, per field, the value with the highest stamp, so whatever order
ops arrive in, both end up with the same project (a last-writer-wins CRDT).
Ops travel over the peer link's reliable signal channel; a digest every few
seconds catches anything that slipped through, and then both send everything.

Audio isn't in the ops: a clip names a file, and whoever lacks the file asks
the other side, which sends it with the engine's file transfer.

Takes are recorded where they're played: each side records its own inputs
against its own playback of the timeline, corrected for its own sound card
delay, so they land on the beat however far apart you are.

Times: clip starts, the loop and the playhead are in beats (so they follow the
tempo, like the grid); offsets into audio and audio clip lengths are in seconds.
Everything about notes is in beats: a pattern clip's length (blen) and offset
into its pattern (boff), a note's start and length within its pattern.

Patterns (FL style): a pattern holds notes for any number of channels; a
channel is a track with an instrument. A pattern clip on any lane plays the
pattern's notes on their channels, repeating it when the clip is longer.
"""
from __future__ import annotations

import base64
import hashlib
import json
import math
import os
import random
import re
import shutil
import string
import threading
import time
import traceback
from pathlib import Path

KINDS = ("meta", "track", "clip", "file", "pattern", "note")
MAX_TRACKS = 32
META_DEFAULTS = {"name": "Untitled", "bpm": 120.0, "beats": 4, "loop": False, "loop_start": 0.0, "loop_end": 16.0,
                 "master_rack": []}
# per field, what the UI may set (anything else in an edit is ignored)
FIELDS = {
    "meta": {"name", "bpm", "beats", "loop", "loop_start", "loop_end", "master_rack"},
    # bus: a mixer track others feed (no clips); out: the bus it goes to (else
    # the master); sends: {bus: level}
    "track": {"name", "order", "owner", "src", "arm", "vol", "pan", "rack", "inst", "seq", "bus", "out", "sends", "del"},
    "clip": {"track", "file", "pattern", "start", "offset", "len", "blen", "boff", "gain", "fade_in", "fade_out", "del"},
    "file": {"frames", "sr", "owner", "del"},
    "pattern": {"name", "len", "color", "del"},
    "note": {"pattern", "track", "start", "len", "key", "vel", "del"},
}


# what a field reads as when it was never set (to undo setting it)
FIELD_DEFAULTS = {
    "meta": META_DEFAULTS,
    "track": {"arm": False, "vol": 0.8, "pan": 0.0, "rack": [], "src": 0, "inst": None, "seq": None,
              "bus": False, "out": None, "sends": {}, "del": False},
    "clip": {"gain": 1.0, "offset": 0.0, "boff": 0.0, "fade_in": 0.0, "fade_out": 0.0, "del": False},
    "file": {"del": False},
    "pattern": {"del": False},
    "note": {"vel": 100, "del": False},
}
# controls you turn rather than click: a run of turns is one undo step
CONTINUOUS = {"vol", "pan", "gain", "bpm", "rack", "inst", "vel", "sends", "master_rack"}
MAX_SENDS = 4
UNDO_DEPTH = 200

# ids and file names end up in paths: only plain names, whatever the partner sends
SAFE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,80}$")


def safe(name) -> bool:
    return isinstance(name, str) and bool(SAFE.match(name)) and ".." not in name


def track_key(tid: str) -> int:
    """The number both sides know a track by in the engine (for notes played
    live): its slot can differ per side, its id can't."""
    return int.from_bytes(hashlib.blake2s(tid.encode(), digest_size=4).digest(), "big") or 1


def new_id(prefix: str = "") -> str:
    return prefix + "".join(random.choices(string.ascii_lowercase + string.digits, k=10))


class Project:
    """The shared document, plus this machine's own view settings (`local`)."""

    def __init__(self, pid: str, folder: Path):
        self.pid = pid
        self.folder = folder
        self.objs: dict[str, dict[str, dict]] = {k: {} for k in KINDS}
        self.stamps: dict[str, dict[str, dict]] = {k: {} for k in KINDS}
        self.clock = 0
        # this machine only: mute/solo per track, click, count-in
        self.local = {"mute": {}, "solo": {}, "click": True, "countin": True}

    @property
    def audio_dir(self) -> Path:
        return self.folder / "audio"

    def meta(self) -> dict:
        return {**META_DEFAULTS, **self.objs["meta"].get("meta", {})}

    def live(self, kind: str) -> dict[str, dict]:
        return {i: o for i, o in self.objs[kind].items() if not o.get("del")}

    def is_empty(self) -> bool:
        return not self.live("clip")

    def apply(self, op) -> bool:
        """Merge one op; True if anything visible changed."""
        try:
            kind, oid, fields = op
        except (TypeError, ValueError):
            return False
        if kind not in KINDS or not isinstance(fields, dict) or not isinstance(oid, str):
            return False
        if kind == "file" and not safe(oid):
            return False
        obj = self.objs[kind].setdefault(oid, {})
        st = self.stamps[kind].setdefault(oid, {})
        changed = False
        for f, v in fields.items():
            if f not in FIELDS[kind]:
                continue
            try:
                value, lam, peer = v
                if f == "file" and not safe(value):
                    continue
                new = (int(lam), str(peer))
            except (TypeError, ValueError):
                continue
            self.clock = max(self.clock, new[0])
            old = st.get(f)
            if old is None or new > tuple(old):
                st[f] = new
                if obj.get(f) != value:
                    obj[f] = value
                    changed = True
        return changed

    def make_op(self, kind: str, oid: str, fields: dict, me: str) -> list:
        self.clock += 1
        return [kind, oid, {f: [v, self.clock, me] for f, v in fields.items()}]

    def all_ops(self) -> list:
        return [[k, i, {f: [v, *self.stamps[k][i][f]] for f, v in o.items()}]
                for k in KINDS for i, o in self.objs[k].items()]

    def digest(self) -> str:
        blob = json.dumps([self.objs, {k: {i: {f: list(s) for f, s in d.items()} for i, d in v.items()}
                                       for k, v in self.stamps.items()}], sort_keys=True)
        return hashlib.sha1(blob.encode()).hexdigest()[:16]

    # --- disk -------------------------------------------------------------------

    def save(self):
        self.folder.mkdir(parents=True, exist_ok=True)
        data = {"pid": self.pid, "clock": self.clock, "objs": self.objs,
                "stamps": {k: {i: {f: list(s) for f, s in d.items()} for i, d in v.items()} for k, v in self.stamps.items()}}
        for name, content in (("project.json", data), ("local.json", self.local)):
            tmp = self.folder / (name + ".tmp")
            tmp.write_text(json.dumps(content), encoding="utf-8")
            os.replace(tmp, self.folder / name)

    @classmethod
    def load(cls, folder: Path) -> "Project":
        data = json.loads((folder / "project.json").read_text(encoding="utf-8"))
        p = cls(data["pid"], folder)
        p.clock = int(data.get("clock", 0))
        for k in KINDS:
            p.objs[k] = data.get("objs", {}).get(k, {})
            p.stamps[k] = {i: {f: tuple(s) for f, s in d.items()} for i, d in data.get("stamps", {}).get(k, {}).items()}
        try:
            p.local.update(json.loads((folder / "local.json").read_text(encoding="utf-8")))
        except (OSError, ValueError):
            pass
        return p


class Daw:
    """The project, kept in step with the partner and with the engine."""

    def __init__(self, engine, root: Path, me: str, rec_offset_ms: float = 0.0):
        self.e = engine
        self.root = root
        self.me = me
        self.rec_offset = rec_offset_ms / 1000.0
        self.lock = threading.RLock()
        self.incoming = root / "_incoming"
        self.incoming.mkdir(parents=True, exist_ok=True)
        engine.file_dir(str(self.incoming))
        self.p = self._open_last()
        self.version = 1
        self.connected = False
        self.partner: str | None = None
        self.partner_pid: str | None = None
        self.conflict: dict | None = None
        self.notices: list[str] = []
        self.ui_signals: list[str] = []
        self.outbox: dict[tuple, list] = {}
        self.last_flush = 0.0
        self.dirty = False
        self.changed_at = 0.0
        self.wanted: dict[str, float] = {}
        self.loaded: set[str] = set()
        self.load_failed: set[str] = set()
        self.peak_cache: dict[str, str] = {}
        self.slots: dict[str, int] = {}
        self.sent_strip: dict = {}
        self.sent_rack: dict[int, list] = {}
        self.sent_inst: dict[int, dict | None] = {}
        self.sent_desc = None
        self.sent_notes = None
        self.sent_keys = None
        self.sent_routes = None
        self.sent_mrack: list = []
        self.pattern_mode: str | None = None  # playing just this pattern, looped (local)
        self.sent_loop = None
        self.sent_click = None
        self.cursor = 0.0  # playhead while stopped, in beats
        self.play_from = 0.0
        self.rec: dict | None = None
        self.undos: list[dict] = []  # your own edits only: undo never takes back the partner's work
        self.redos: list[dict] = []
        self.last_sum = 0.0
        self.mismatches = 0
        self.last_snap = 0.0
        self.last_files = 0.0
        self.last_hello = 0.0
        self.partner_here: dict | None = None  # where the partner is (presence)
        self.partner_here_n = 0.0
        self._running = True
        self._push()
        self.thread = threading.Thread(target=self._loop, name="duet-daw", daemon=True)
        self.thread.start()

    # --- projects on disk -----------------------------------------------------

    def _open_last(self) -> Project:
        last = None
        try:
            last = (self.root / "last.txt").read_text(encoding="utf-8").strip()
        except OSError:
            pass
        if last and (self.root / last / "project.json").exists():
            try:
                return Project.load(self.root / last)
            except (OSError, ValueError, KeyError):
                traceback.print_exc()
        return self._fresh("My first song")

    def _fresh(self, name: str) -> Project:
        pid = new_id("p")
        p = Project(pid, self.root / pid)
        p.apply(p.make_op("meta", "meta", {**META_DEFAULTS, "name": name}, self.me))
        p.apply(p.make_op("track", new_id("t"), {"name": "Track 1", "order": 1.0, "owner": self.me, "src": 0,
                                                 "arm": False, "vol": 0.8, "pan": 0.0, "rack": []}, self.me))
        p.save()
        self._remember(pid)
        return p

    def _remember(self, pid: str):
        try:
            self.root.mkdir(parents=True, exist_ok=True)
            (self.root / "last.txt").write_text(pid, encoding="utf-8")
        except OSError:
            pass

    def projects(self) -> list[dict]:
        out = []
        for d in self.root.iterdir() if self.root.exists() else []:
            f = d / "project.json"
            if not f.exists():
                continue
            try:
                p = Project.load(d)
            except (OSError, ValueError, KeyError):
                continue
            out.append({"pid": p.pid, "name": p.meta()["name"], "modified": f.stat().st_mtime,
                        "clips": len(p.live("clip")), "current": p.pid == self.p.pid})
        return sorted(out, key=lambda x: -x["modified"])

    def _switch(self, p: Project):
        """Make `p` the open project (the old one is saved)."""
        if self.rec or self._playing():
            self._stop(broadcast=False)
        self.p.save()
        for key in list(self.loaded):
            self.e.daw_unload(key)
        self.loaded.clear()
        self.load_failed.clear()
        self.wanted.clear()
        self.sent_desc = None
        self.pattern_mode = None
        self.cursor = 0.0
        self.undos.clear()
        self.redos.clear()
        self.partner_here = None
        self.p = p
        self._remember(p.pid)
        self._changed()

    def new_project(self, name: str = "New song"):
        with self.lock:
            self._switch(self._fresh(name or "New song"))
            self._share_project()

    def open_project(self, pid: str):
        with self.lock:
            if pid == self.p.pid or not safe(pid):
                return
            self._switch(Project.load(self.root / pid))
            self._share_project()

    def _share_project(self):
        """We changed projects: the partner comes along."""
        self.conflict = None
        if self.connected:
            self._send({"t": "snap", "adopt": True, "name": self.p.meta()["name"], "ops": self.p.all_ops()})
            self.partner_pid = self.p.pid

    # --- edits ---------------------------------------------------------------------

    def edit(self, changes: list) -> dict:
        """Changes from the UI: [[kind, id, {field: value}]]. A new id makes a new object."""
        with self.lock:
            self._do(changes)
            return self.view()

    def _do(self, changes: list, undoable: bool = True):
        """Make local changes: stamp, apply, send, and remember how to undo them."""
        forward, inverse = [], []
        for ch in changes:
            kind, oid, fields = ch
            if kind not in KINDS or not isinstance(fields, dict):
                continue
            oid = str(oid)
            fields = {f: v for f, v in fields.items() if f in FIELDS[kind]}
            if not fields:
                continue
            before = self._before(kind, oid, fields)
            if "del" in before and "del" not in fields:
                fields["del"] = False  # a new object: so redoing it after an undo brings it back
            forward.append([kind, oid, fields])
            inverse.append([kind, oid, before])
        if not forward:
            return
        self._apply([self.p.make_op(k, i, f, self.me) for k, i, f in forward], local=True)
        if not undoable:
            return
        keys = sorted((k, i, f) for k, i, fs in forward for f in fs)
        top = self.undos[-1] if self.undos else None
        now = time.monotonic()
        if (top and top["keys"] == keys and now - top["t"] < 1.5
                and all(f in CONTINUOUS for _, _, f in keys)):
            top["forward"], top["t"] = forward, now  # still turning the same knob
        else:
            self.undos.append({"forward": forward, "inverse": inverse, "keys": keys, "t": now})
            del self.undos[:-UNDO_DEPTH]
        self.redos.clear()

    def _before(self, kind: str, oid: str, fields: dict) -> dict:
        """What `fields` of an object are now (for an object that doesn't exist yet: deleted)."""
        obj = self.p.objs[kind].get(oid)
        if obj is None:
            return {"del": True}
        out = {}
        for f in fields:
            if f in obj:
                out[f] = json.loads(json.dumps(obj[f]))
            elif f in FIELD_DEFAULTS[kind]:
                out[f] = json.loads(json.dumps(FIELD_DEFAULTS[kind][f]))
        return out

    def undo(self) -> dict:
        with self.lock:
            if self.undos:
                step = self.undos.pop()
                self._apply([self.p.make_op(k, i, f, self.me) for k, i, f in reversed(step["inverse"])], local=True)
                self.redos.append(step)
            return self.view()

    def redo(self) -> dict:
        with self.lock:
            if self.redos:
                step = self.redos.pop()
                self._apply([self.p.make_op(k, i, f, self.me) for k, i, f in step["forward"]], local=True)
                step["t"] = 0.0  # never merge into a redone step
                self.undos.append(step)
            return self.view()

    def set_local(self, key: str, value, track: str | None = None):
        """This machine only: mute/solo of a track, click, count-in."""
        with self.lock:
            if key in ("mute", "solo") and track:
                self.p.local[key][track] = bool(value)
            elif key in ("click", "countin"):
                self.p.local[key] = bool(value)
            self._changed()

    def track_rack_changed(self, slot: int):
        """The UI changed the effects on track strip T<slot> through the mixer
        calls: put the new rack in the project (and so on the partner's side)."""
        with self.lock:
            tid = next((t for t, s in self.slots.items() if s == slot), None)
            if tid is None:
                return
            rack = self._song_rack(self.e.mixer()["tracks"][slot]["rack"])
            self.sent_rack[slot] = self._engine_rack(rack)
            self._do([["track", tid, {"rack": rack}]])

    def master_rack_changed(self):
        """The same for the master's effects (part of the song too)."""
        with self.lock:
            rack = self._song_rack(self.e.mixer()["master_strip"]["rack"])
            self.sent_mrack = self._engine_rack(rack)
            self._do([["meta", "meta", {"master_rack": rack}]])

    def _song_rack(self, rack: list) -> list:
        """A rack as the engine reports it, as the song keeps it: a sidechain
        names the track it listens to (slots differ per side)."""
        byslot = {s: t for t, s in self.slots.items()}
        out = []
        for x in rack:
            x = clean_slot(x)
            if "key" in x:
                x["key"] = byslot.get(x["key"]) if isinstance(x["key"], int) else None
            out.append(x)
        return out

    def _engine_rack(self, rack: list) -> list:
        out = []
        for x in rack:
            x = json.loads(json.dumps(x))
            if "key" in x:
                x["key"] = self.slots.get(x["key"], -1) if x["key"] else -1
            out.append(x)
        return out

    def sync_rack_states(self, mixer: dict):
        """Plugin settings changed in a plugin's own window don't pass through
        Duet: with the freshly read mixer, put any that changed into the song."""
        with self.lock:
            changes = []
            tracks = mixer["tracks"]
            rack = self._song_rack(mixer["master_strip"]["rack"])
            if any(x.get("vst") for x in rack) and rack != self.p.meta().get("master_rack"):
                self.sent_mrack = self._engine_rack(rack)
                changes.append(["meta", "meta", {"master_rack": rack}])
            for tid, slot in self.slots.items():
                rack = self._song_rack(tracks[slot]["rack"])
                if any(x.get("vst") for x in rack) and rack != self.p.live("track").get(tid, {}).get("rack"):
                    self.sent_rack[slot] = self._engine_rack(rack)
                    changes.append(["track", tid, {"rack": rack}])
                inst = clean_inst(tracks[slot].get("inst"))
                if inst and inst.get("vst") and inst != self.p.live("track").get(tid, {}).get("inst"):
                    self.sent_inst[slot] = json.loads(json.dumps(inst))
                    changes.append(["track", tid, {"inst": inst}])
            self._do(changes, undoable=False)

    def _apply(self, ops: list, local: bool):
        changed = False
        for op in ops:
            changed |= self.p.apply(op)
            if local:
                kind, oid, fields = op
                self.outbox.setdefault((kind, oid), [kind, oid, {}])[2].update(fields)
        if changed or local:
            self._changed()

    def _changed(self):
        self.version += 1
        self.dirty = True
        self.changed_at = time.monotonic()
        self._push()

    # --- transport -----------------------------------------------------------------

    def _spb(self) -> float:
        return 60.0 / max(20.0, float(self.p.meta()["bpm"]))

    def _playing(self) -> bool:
        return bool(self.e.state()["daw_playing"])

    def play(self, pos: float | None = None):
        with self.lock:
            self._start(self.cursor if pos is None else float(pos), rec=False, countin=False, broadcast=True)

    def record(self, pos: float | None = None) -> dict:
        with self.lock:
            armed = [t for t in self.p.live("track").values() if t.get("arm")]
            if not armed:
                return {"error": "Arm a track first: the red dot on the track."}
            if self.rec:
                return {"error": "Already recording."}
            self._start(self.cursor if pos is None else float(pos), rec=True,
                        countin=bool(self.p.local.get("countin")), broadcast=True)
            return {"ok": True}

    def _start(self, pos: float, rec: bool, countin: bool, broadcast: bool, delay: float = 0.0):
        if self.rec or self._playing():
            self._stop(broadcast=False)
        m = self.p.meta()
        spb = self._spb()
        bar = spb * int(m["beats"])
        at = pos * spb + delay  # seconds; a partner's start is shifted by the one-way delay
        start = at - bar if countin else at
        until = at if countin else None
        self.play_from = pos
        sources = {}
        if rec:
            live = self.p.live("track")
            # armed audio tracks record your input; armed channels record the notes you play
            mine = {tid: t for tid, t in live.items()
                    if t.get("arm") and t.get("owner") == self.me and tid in self.slots and not t.get("inst")}
            channels = [tid for tid, t in live.items() if t.get("arm") and t.get("inst") and tid in self.slots]
            for tid, t in mine.items():
                src = int(t.get("src", 0))
                if src not in sources:
                    sources[src] = new_id("take-") + ".wav"
            self.p.audio_dir.mkdir(parents=True, exist_ok=True)
            self.rec = {"punch": at, "sources": sources, "tracks": {tid: int(t.get("src", 0)) for tid, t in mine.items()},
                        "channels": channels}
        if sources or (self.rec and self.rec["channels"]):
            self.e.daw_record([(src, key, str(self.p.audio_dir / key)) for src, key in sources.items()], start, until)
        else:
            self.e.daw_play(start, until)
        if broadcast:
            self._send({"t": "tp", "play": True, "rec": rec, "pos": pos, "countin": countin})
        self.version += 1

    def stop(self):
        with self.lock:
            self._stop(broadcast=True)

    def _stop(self, broadcast: bool):
        stop_at = self.e.state()["daw_pos"]
        takes = self.e.daw_stop()
        played = self.e.daw_rec_notes()
        rec, self.rec = self.rec, None
        if self.pattern_mode:
            self.pattern_mode = None
            self._push()
        if rec and takes:
            spb = self._spb()
            changes = []
            for take in takes:
                if take["error"] or not take["frames"]:
                    if take["error"]:
                        self.notices.append("Recording failed: " + take["error"])
                    continue
                key = take["key"]
                self.peak_cache[key] = base64.b64encode(take["peaks"]).decode()
                self.loaded.add(key)
                changes.append(["file", key, {"frames": take["frames"], "sr": take["sr"], "owner": self.me}])
                # where the take's first sample belongs on the timeline: what the
                # sound cards added is taken off, so it lines up with what you heard
                first = take["first"] - take["comp"] - self.rec_offset
                start = max(rec["punch"], first)
                offset = start - first
                length = take["frames"] / take["sr"] - offset
                if length <= 0.01:
                    continue
                for tid, src in rec["tracks"].items():
                    if src == take["src"]:
                        changes.append(["clip", new_id("c"), {
                            "track": tid, "file": key, "start": start / spb, "offset": offset, "len": length,
                            "gain": 1.0, "fade_in": 0.0, "fade_out": 0.0}])
                if take["dropped"]:
                    self.notices.append("The recording skipped a little (the disk couldn't keep up).")
            self._do(changes)  # one undo step takes the whole take back out
            self._flush()
        if rec and played:
            self._take_notes(rec, played, stop_at)
        self.cursor = self.play_from  # back to where you started, like FL
        self.e.daw_locate(self.cursor * self._spb())
        if broadcast:
            self._send({"t": "tp", "play": False})
        self.version += 1

    def _take_notes(self, rec: dict, played: list, stop_at: float):
        """Notes played on armed channels while recording become a new pattern
        per channel, placed where the recording started."""
        spb, beats = self._spb(), int(self.p.meta()["beats"])
        st = self.e.state()
        # you played to what you heard: after the sound card and the master rack
        lat = ((st.get("lat_out_ms") or 0.0) + (st.get("master_latency_ms") or 0.0)) / 1000.0
        punch = rec["punch"] / spb
        tracks = self.p.live("track")
        changes = []
        for tid in rec.get("channels", []):
            slot = self.slots.get(tid)
            evs = [ev for ev in played if ev[0] == slot]
            if slot is None or not evs:
                continue
            down, pedal, held, notes = {}, False, set(), []

            def end(key, at):
                b0, vel = down.pop(key)
                notes.append((b0, max(0.02, at - b0), key, vel))
            for _slot, kind, key, vel, t in evs:
                at = (t - lat) / spb - punch
                if kind == "on":
                    if key in down:
                        end(key, at)
                    down[key] = (at, vel)
                    held.discard(key)
                elif kind == "off" and key in down:
                    if pedal:
                        held.add(key)
                    else:
                        end(key, at)
                elif kind == "pedal down":
                    pedal = True
                elif kind == "pedal up":
                    pedal = False
                    for k in [k for k in held if k in down]:
                        end(k, at)
                    held.clear()
            last = (stop_at - lat) / spb - punch
            for k in list(down):
                end(k, last)
            notes = [n for n in notes if n[0] >= -1e-6]  # not what was played during the count-in
            if not notes:
                continue
            length = max(beats, math.ceil(max(b + d for b, d, _, _ in notes) / beats - 1e-9) * beats)
            pid = new_id("p")
            name = f"{tracks[tid].get('name') or 'Channel'} take"
            changes.append(["pattern", pid, {"name": name, "len": float(length)}])
            for b, d, key, vel in notes:
                changes.append(["note", new_id("n"), {"pattern": pid, "track": tid, "start": max(0.0, b), "len": d,
                                                      "key": int(key), "vel": int(vel)}])
            changes.append(["clip", new_id("c"), {"track": tid, "pattern": pid, "start": punch, "blen": float(length), "boff": 0.0}])
        if changes:
            self._do(changes)
            self._flush()

    def locate(self, pos: float):
        with self.lock:
            self.cursor = max(-64.0, float(pos))
            if self._playing():
                self.e.daw_locate(self.cursor * self._spb())
            self.version += 1

    # --- export --------------------------------------------------------------------

    def export(self, mix: str | None, stems_dir: str | None, loop: bool) -> dict:
        """Render the song (or just the loop) to WAV: the mix, and with stems_dir
        every track on its own too. Runs in the background (engine.daw_export_status)."""
        with self.lock:
            spb, m = self._spb(), self.p.meta()
            clips = list(self.p.live("clip").values())
            if loop and m["loop"] and m["loop_end"] > m["loop_start"]:
                start, end = m["loop_start"] * spb, m["loop_end"] * spb  # exact, so it loops
            else:
                if not clips:
                    return {"error": "Nothing to export yet: record something first."}
                start = 0.0
                end = max(float(c.get("start", 0)) * spb + (float(c.get("blen") or 0) * spb if c.get("pattern")
                                                            else float(c.get("len", 0))) for c in clips) + 2.0  # + tails
            stems, files = [], [mix] if mix else []
            if stems_dir:
                song = file_name(m["name"])
                tracks = sorted(((self.p.live("track")[tid], slot) for tid, slot in self.slots.items()),
                                key=lambda ts: float(ts[0].get("order", 0)))
                for n, (t, slot) in enumerate(tracks, 1):
                    path = str(Path(stems_dir) / f"{song} - {n:02d} {file_name(t.get('name') or 'Track')}.wav")
                    stems.append((slot, path))
                    files.append(path)
            missing = sum(1 for c in clips if c.get("file") and c.get("file") not in self.loaded)
            self.e.mixer(True)  # plugin settings changed in their own windows: into the render too
            try:
                self.e.daw_export(mix, stems, start, end)
            except Exception as exc:
                return {"error": str(exc)}
            note = (f"{missing} clip{'s are' if missing != 1 else ' is'} still on the way from your partner "
                    "and left out.") if missing else ""
            return {"ok": True, "files": files, "note": note}

    # --- engine --------------------------------------------------------------------

    def _push(self):
        """Bring the engine in line with the project (only what changed)."""
        e, p = self.e, self.p
        tracks = sorted(p.live("track").items(), key=lambda kv: (float(kv[1].get("order", 0)), kv[0]))
        alive = {tid for tid, _ in tracks}
        for tid in [t for t in self.slots if t not in alive]:
            slot = self.slots.pop(tid)
            e.strip_set(f"T{slot}", "active", 0)
            e.rack_load(f"T{slot}", [])
            if self.sent_inst.pop(slot, None):
                e.inst_set(f"T{slot}", None)
            self.sent_rack.pop(slot, None)
            for k in [k for k in self.sent_strip if k[0] == slot]:
                del self.sent_strip[k]
        used = set(self.slots.values())
        for tid, _ in tracks:
            if tid not in self.slots:
                free = next((s for s in range(MAX_TRACKS) if s not in used), None)
                if free is None:
                    break
                self.slots[tid] = free
                used.add(free)
        keys = [0] * MAX_TRACKS
        for tid, slot in self.slots.items():
            keys[slot] = track_key(tid)
        if keys != self.sent_keys:
            e.track_keys(keys)
            self.sent_keys = keys
        byid = dict(tracks)
        for tid, slot in self.slots.items():
            t = byid[tid]
            want = {"active": 1, "gain": 1.0, "fader": float(t.get("vol", 0.8)), "pan": float(t.get("pan", 0.0)),
                    "mute": float(bool(p.local["mute"].get(tid))), "solo": float(bool(p.local["solo"].get(tid)))}
            for k, v in want.items():
                if self.sent_strip.get((slot, k)) != v:
                    e.strip_set(f"T{slot}", k, v)
                    self.sent_strip[(slot, k)] = v
            rack = self._engine_rack(t.get("rack") or [])
            if self.sent_rack.get(slot) != rack:
                self._apply_rack(f"T{slot}", self.sent_rack.get(slot) or [], rack)
                self.sent_rack[slot] = rack
            inst = t.get("inst") or None
            if self.sent_inst.get(slot) != inst:
                self._apply_inst(slot, self.sent_inst.get(slot), inst)
                self.sent_inst[slot] = json.loads(json.dumps(inst))
        # where each track goes: a bus (or the master), and its sends
        routes = [(-1, []) for _ in range(MAX_TRACKS)]
        for tid, slot in self.slots.items():
            t = byid[tid]
            bus = lambda b: b != tid and b in self.slots and bool(byid.get(b, {}).get("bus"))
            out = self.slots[t["out"]] if bus(t.get("out")) else -1
            sends = [(self.slots[b], float(v)) for b, v in sorted((t.get("sends") or {}).items())
                     if bus(b) and isinstance(v, (int, float)) and v > 0][:MAX_SENDS]
            routes[slot] = (out, sends)
        if routes != self.sent_routes:
            e.daw_routing(routes)
            self.sent_routes = routes
        mrack = self._engine_rack(p.meta().get("master_rack") or [])
        if mrack != self.sent_mrack:
            self._apply_rack("M", self.sent_mrack, mrack)
            self.sent_mrack = mrack
        spb = self._spb()
        if self.pattern_mode and self.pattern_mode not in p.live("pattern"):
            self.pattern_mode = None
        desc = [[] for _ in range(MAX_TRACKS)]
        for c in ([] if self.pattern_mode else p.live("clip").values()):
            slot = self.slots.get(c.get("track"))
            key = c.get("file")
            if slot is None or key not in self.loaded:
                continue
            desc[slot].append((key, float(c.get("start", 0)) * spb, float(c.get("offset", 0)), float(c.get("len", 0)),
                               float(c.get("gain", 1.0)), float(c.get("fade_in", 0)), float(c.get("fade_out", 0))))
        notes = self._notes(spb)
        if desc != self.sent_desc or notes != self.sent_notes:
            e.daw_timeline(desc, notes)
            self.sent_desc, self.sent_notes = desc, notes
        m = p.meta()
        loop = (bool(m["loop"]), float(m["loop_start"]) * spb, float(m["loop_end"]) * spb)
        if self.pattern_mode:
            loop = (True, 0.0, self._pattern_len(self.pattern_mode) * spb)
        if loop != self.sent_loop:
            e.daw_loop(*loop)
            self.sent_loop = loop
        click = (bool(p.local.get("click")), float(m["bpm"]), int(m["beats"]))
        if click != self.sent_click:
            e.daw_click(*click)
            self.sent_click = click

    def _pattern_len(self, pid: str) -> float:
        return max(0.25, float(self.p.live("pattern").get(pid, {}).get("len") or 4 * int(self.p.meta()["beats"])))

    def _notes(self, spb: float) -> list:
        """Every channel's notes in seconds: patterns unrolled where their clips
        are (repeating to fill a clip), or the open pattern alone in pattern mode."""
        p = self.p
        out = [[] for _ in range(MAX_TRACKS)]
        by_pat: dict[str, list] = {}
        for n in p.live("note").values():
            by_pat.setdefault(n.get("pattern"), []).append(n)

        def put(n, t, until):
            slot = self.slots.get(n.get("track"))
            if slot is None:
                return
            dur = min(float(n.get("len", 0.25)), until - t)
            if dur > 1e-6:
                out[slot].append((t * spb, dur * spb, int(n.get("key", 60)) & 127, max(1, min(127, int(n.get("vel", 100))))))

        if self.pattern_mode:
            plen = self._pattern_len(self.pattern_mode)
            for n in by_pat.get(self.pattern_mode, []):
                t = float(n.get("start", 0))
                if 0 <= t < plen:
                    put(n, t, plen)
            return out
        pats = p.live("pattern")
        for c in p.live("clip").values():
            pid = c.get("pattern")
            if pid not in pats or not by_pat.get(pid):
                continue
            plen = self._pattern_len(pid)
            start, boff = float(c.get("start", 0)), float(c.get("boff", 0))
            end = start + float(c.get("blen") or plen)
            origin = start - boff  # where beat 0 of the pattern's first round falls
            k = math.floor(boff / plen)
            while origin + k * plen < end:
                base = origin + k * plen
                for n in by_pat[pid]:
                    t = base + float(n.get("start", 0))
                    if start - 1e-9 <= t < end and float(n.get("start", 0)) < plen:
                        put(n, t, end)
                k += 1
        return out

    def _apply_inst(self, slot: int, old: dict | None, new: dict | None):
        t = f"T{slot}"
        try:
            if not new:
                self.e.inst_set(t, None)
                return
            same = (old and old.get("kind") == new.get("kind") and old.get("pads") == new.get("pads")
                    and (old.get("vst") or {}).get("cid") == (new.get("vst") or {}).get("cid"))
            if not same:
                self.e.inst_set(t, new["kind"], None, new)
                return
            for k, v in new.items():
                if k not in ("kind", "pads", "vst") and old.get(k) != v and isinstance(v, (int, float)):
                    self.e.inst_param(t, k, float(v))
        except Exception as exc:  # e.g. a VST instrument the other side has and we don't
            self.notices.append(f"Couldn't load the instrument on a track: {exc}")

    def track_inst_changed(self, slot: int):
        """The UI changed a song track's instrument (or its knobs, pads) through
        the instrument calls: put it in the song."""
        with self.lock:
            tid = next((t for t, s in self.slots.items() if s == slot), None)
            if tid is None:
                return
            inst = clean_inst(self.e.mixer()["tracks"][slot]["inst"])
            self.sent_inst[slot] = json.loads(json.dumps(inst))
            self._do([["track", tid, {"inst": inst}]])

    def play_pattern(self, pid: str):
        """Play one pattern on its own, looped (just here, not for the partner)."""
        with self.lock:
            if pid not in self.p.live("pattern"):
                return
            if self.rec or self._playing():
                self._stop(broadcast=True)
            self.pattern_mode = pid
            self._push()
            self.play_from = self.cursor
            self.e.daw_play(0.0)
            self.version += 1

    def _apply_rack(self, t: str, old: list, new: list):
        same_shape = len(old) == len(new) and all(
            a.get("kind") == b.get("kind") and a.get("model") == b.get("model")
            and (a.get("vst") or {}).get("cid") == (b.get("vst") or {}).get("cid")
            for a, b in zip(old, new))
        if not same_shape:
            self.e.rack_load(t, new)
            # a plugin the other side has and we don't: its slot lets the sound
            # through, and says so
            rack = self.e.mixer()["master_strip" if t == "M" else "tracks"]
            rack = rack["rack"] if t == "M" else rack[int(t[1:])]["rack"]
            for x in rack:
                if x.get("error"):
                    name = (x.get("vst") or {}).get("name") or x.get("model") or x["kind"]
                    self.notices.append(f"{name} isn't on this computer: that effect is skipped here.")
            return
        for pos, (a, b) in enumerate(zip(old, new)):
            for k, v in b.items():
                if k in ("kind", "model", "vst") or a.get(k) == v:
                    continue
                try:
                    self.e.rack_set(t, pos, k, float(v) if k != "on" else (1.0 if v else 0.0))
                except Exception:
                    pass

    # --- the partner -----------------------------------------------------------------

    def _send(self, msg: dict):
        if not self.connected and msg.get("t") not in ("hello",):
            return
        msg = {"app": "daw", "pid": self.p.pid, **msg}
        self.e.send_signal(json.dumps(msg, separators=(",", ":")))

    def _flush(self):
        if self.outbox and self.connected and self.partner_pid == self.p.pid:
            self._send({"t": "ops", "ops": list(self.outbox.values())})
        self.outbox.clear()
        self.last_flush = time.monotonic()

    def _on_connect(self):
        self.last_hello = time.monotonic()
        self.partner_pid = None
        self.partner_here, self.partner_here_n = None, 0.0
        self.mismatches = 0
        self.wanted.clear()
        self._send({"t": "hello", "peer": self.me, "name": self.p.meta()["name"], "empty": self.p.is_empty()})

    def _on_msg(self, m: dict):
        t, pid = m.get("t"), m.get("pid")
        if not safe(pid):
            return
        if t == "hello":
            self.partner = m.get("peer")
            if pid == self.p.pid:
                self.partner_pid = pid
                self.conflict = None
                self._send({"t": "snap", "adopt": False, "ops": self.p.all_ops()})
            elif self.p.is_empty() and (not m.get("empty") or self.p.pid < pid):
                self._send({"t": "pull"})
            elif m.get("empty"):
                pass  # they'll pull ours
            else:
                self.conflict = {"pid": pid, "name": m.get("name") or "their song"}
            self.version += 1
        elif t == "pull":
            self._share_project()
        elif t == "snap":
            if m.get("adopt") and pid != self.p.pid:
                folder = self.root / pid
                p = Project.load(folder) if (folder / "project.json").exists() else Project(pid, folder)
                self._switch(p)
                self.notices.append(f"Opened your partner's song: {m.get('name') or 'untitled'}")
            if pid == self.p.pid:
                self.partner_pid = pid
                self.conflict = None
                self.last_snap = time.monotonic()
                self._apply(m.get("ops") or [], local=False)
        elif t == "ops" and pid == self.p.pid:
            self._apply(m.get("ops") or [], local=False)
        elif t == "want" and pid == self.p.pid:
            name = m.get("file")
            if not safe(name):
                return
            path = self.p.audio_dir / name
            if name in self.p.objs["file"] and path.exists():
                self.e.file_send(name, str(path))
        elif t == "tp" and pid == self.p.pid:
            if m.get("play"):
                rtt = self.e.state().get("rtt") or 0.0
                self._start(float(m.get("pos", 0)), rec=bool(m.get("rec")), countin=bool(m.get("countin")),
                            broadcast=False, delay=rtt / 2000.0)
            else:
                self._stop(broadcast=False)
        elif t == "here" and pid == self.p.pid:
            n = m.get("n")
            if isinstance(n, (int, float)) and n > self.partner_here_n:  # a resend can overtake a newer one
                self.partner_here_n = n
                self.partner_here = {**clean_here(m), "pos": finite(m.get("pos"), 0.0), "at": time.monotonic()}
        elif t == "sum" and pid == self.p.pid:
            if m.get("d") != self.p.digest():
                self.mismatches += 1
                if self.mismatches >= 2 and time.monotonic() - self.last_snap > 4:
                    self.mismatches = 0
                    self.last_snap = time.monotonic()
                    self._send({"t": "snap", "adopt": False, "ops": self.p.all_ops()})
            else:
                self.mismatches = 0

    # --- presence: seeing each other work ------------------------------------------

    def set_presence(self, here: dict):
        """Where you are (view, pattern, channel, selection, pointer, scroll), for
        the partner to see in your colour. The UI sends it when it changes, and
        every couple of seconds so it doesn't go stale."""
        with self.lock:
            if self.connected and self.partner_pid == self.p.pid:
                self._send({"t": "here", "n": time.time(), **clean_here(here), "pos": self.cursor})

    def _partner_here(self) -> dict | None:
        h = self.partner_here
        if not h or not self.connected or self.partner_pid != self.p.pid or time.monotonic() - h["at"] > 6:
            return None
        return {k: v for k, v in h.items() if k != "at"}

    def resolve(self, choice: str):
        """Different songs open on both sides: 'theirs' opens theirs, 'mine' sends mine."""
        with self.lock:
            if choice == "theirs":
                self._send({"t": "pull"})
            elif choice == "mine":
                self._share_project()
            self.conflict = None
            self.version += 1

    # --- background: signals, files, saving -----------------------------------------

    def _loop(self):
        while self._running:
            try:
                self._tick()
            except Exception:
                traceback.print_exc()
            time.sleep(0.03)

    def _tick(self):
        e = self.e
        # the connection first: a message that arrives with it may need an answer
        conn = e.connected
        with self.lock:
            if conn and not self.connected:
                self.connected = True
                self._on_connect()
            elif not conn and self.connected:
                self.connected = False
                self.partner_pid = None
                self.version += 1
        for raw in e.drain_signals():
            try:
                msg = json.loads(raw)
            except ValueError:
                msg = None
            if isinstance(msg, dict) and msg.get("app") == "daw":
                with self.lock:
                    self._on_msg(msg)
            else:
                with self.lock:
                    self.ui_signals.append(raw)
        with self.lock:
            now = time.monotonic()
            if conn and self.partner_pid != self.p.pid and not self.conflict and now - self.last_hello > 5:
                self._on_connect()  # not in the same song yet: say hello again
            if self.outbox and now - self.last_flush >= 0.08:
                self._flush()
            self._files(now)
            if conn and self.partner_pid == self.p.pid and now - self.last_sum > 5:
                self.last_sum = now
                self._send({"t": "sum", "d": self.p.digest()})
            if self.dirty and now - self.changed_at > 1.0:
                self.dirty = False
                try:
                    self.p.save()
                except OSError:
                    traceback.print_exc()

    def _files(self, now: float):
        received, _sent, failed = self.e.file_drain()
        for name, path in received:
            self.wanted.pop(name, None)
            if safe(name) and name in self.p.objs["file"]:
                self._adopt_file(name, Path(path))
        for name in failed:
            if name in self.wanted:
                self.wanted[name] = now - 25  # ask again soon
        if now - self.last_files < 0.5:
            return
        self.last_files = now
        needed = {c.get("file") for c in self.p.live("clip").values()}
        for name in sorted(n for n in needed if safe(n)):
            path = self.p.audio_dir / name
            if not path.exists() and (self.incoming / name).exists():
                self._adopt_file(name, self.incoming / name)
            if path.exists():
                if name not in self.loaded and name not in self.load_failed:
                    self._load(name, path)
                    return  # one per round: loading big takes takes a moment
            elif self.connected and self.partner_pid == self.p.pid and now - self.wanted.get(name, -1e9) > 30:
                self.wanted[name] = now
                self._send({"t": "want", "file": name})

    def _adopt_file(self, name: str, src: Path):
        dst = self.p.audio_dir / name
        try:
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(src), str(dst))
        except OSError:
            traceback.print_exc()
            return
        self._load(name, dst)

    def _load(self, name: str, path: Path):
        try:
            _frames, _sr, peaks = self.e.daw_load(name, str(path))
        except Exception as exc:
            self.load_failed.add(name)
            self.notices.append(f"Couldn't read {name}: {exc}")
            return
        self.peak_cache[name] = base64.b64encode(peaks).decode()
        self.loaded.add(name)
        self.version += 1
        self._push()

    # --- for the UI ------------------------------------------------------------------

    def take_ui_signals(self) -> list[str]:
        with self.lock:
            out, self.ui_signals = self.ui_signals, []
            return out

    def peaks(self, name: str) -> str | None:
        with self.lock:
            return self.peak_cache.get(name)

    def view(self) -> dict:
        """The whole project for the UI."""
        with self.lock:
            p = self.p
            tracks = sorted(({"id": i, **t} for i, t in p.live("track").items()),
                            key=lambda t: (float(t.get("order", 0)), t["id"]))
            for t in tracks:
                t["slot"] = self.slots.get(t["id"])
                t["mine"] = t.get("owner") == self.me
                t["mute"] = bool(p.local["mute"].get(t["id"]))
                t["solo"] = bool(p.local["solo"].get(t["id"]))
                t["bus"] = bool(t.get("bus"))
            files = {}
            for name, f in p.live("file").items():
                files[name] = {**f, "here": name in self.loaded, "mine": f.get("owner") == self.me}
            return {
                "pid": p.pid, "meta": p.meta(), "tracks": [t for t in tracks if not t["bus"]],
                "buses": [t for t in tracks if t["bus"]],
                "clips": [{"id": i, **c} for i, c in p.live("clip").items()],
                "files": files, "me": self.me, "partner": self.partner,
                "local": {"click": p.local.get("click"), "countin": p.local.get("countin")},
                "patterns": sorted(({"id": i, **x} for i, x in p.live("pattern").items()), key=lambda x: str(x.get("name", ""))),
                "notes": [{"id": i, **n} for i, n in p.live("note").items()],
                "patternMode": self.pattern_mode,
                "conflict": self.conflict, "shared": self.connected and self.partner_pid == p.pid,
                "undo": bool(self.undos), "redo": bool(self.redos),
            }

    def status(self, s: dict) -> dict:
        """The fast-changing part, with every state poll."""
        with self.lock:
            if self.rec and not s["daw_recording"] and not s["daw_playing"]:
                self._stop(broadcast=True)  # the engine stopped by itself (sound card restart)
            spb = self._spb()
            playing = bool(s["daw_playing"])
            levels = {tid: s["track_levels"][slot] for tid, slot in self.slots.items()}
            st = self.e.file_status()
            xfer = {n: [d, tot, "out"] for n, d, tot in st["out"]}
            xfer.update({n: [d, tot, "in"] for n, d, tot in st["in"]})
            for n in self.wanted:
                xfer.setdefault(n, [0, 0, "in"])
            notices, self.notices = self.notices, []
            # notes the partner is playing live, by track
            live: dict[str, list[int]] = {}
            byslot = {slot: tid for tid, slot in self.slots.items()}
            for slot, key in self.e.partner_notes:
                if slot in byslot:
                    live.setdefault(byslot[slot], []).append(key)
            return {"v": self.version, "pos": s["daw_pos"] / spb if playing else self.cursor, "playing": playing,
                    "recording": bool(s["daw_recording"]) or bool(self.rec and playing),
                    "recFrom": self.rec["punch"] / spb if self.rec else None,
                    "levels": levels, "xfer": xfer, "notices": notices,
                    "partnerHere": self._partner_here(), "partnerNotes": live}

    def close(self):
        self._running = False
        with self.lock:
            if self.rec:
                self._stop(broadcast=False)
            try:
                self.p.save()
            except OSError:
                pass


VIEWS = ("song", "seq", "mixer")


def finite(x, default=None, lo=-1e6, hi=1e9):
    """A plain number within reason, or the default."""
    if isinstance(x, (int, float)) and not isinstance(x, bool) and math.isfinite(x) and lo <= x <= hi:
        return float(x)
    return default


def clean_here(x) -> dict:
    """What a presence message may hold: a known view, plain ids, sane numbers
    (it comes from the partner, and goes on to the page)."""
    x = x if isinstance(x, dict) else {}
    pick = lambda v: v if safe(v) else None
    ids = lambda v: [i for i in v[:64] if safe(i)] if isinstance(v, list) else []
    out = {"view": x["view"] if x.get("view") in VIEWS else "song",
           "pattern": pick(x.get("pattern")), "track": pick(x.get("track")),
           "sel": ids(x.get("sel")), "notes": ids(x.get("notes")), "ptr": None, "scroll": None, "roll": None}
    ptr = x.get("ptr")
    if isinstance(ptr, dict):
        beat = finite(ptr.get("beat"), lo=-1e3, hi=1e6)
        if beat is not None and ptr.get("in") == "song" and safe(ptr.get("track")):
            out["ptr"] = {"in": "song", "beat": beat, "track": ptr["track"]}
        elif beat is not None and ptr.get("in") == "roll" and finite(ptr.get("key"), lo=0, hi=127) is not None:
            out["ptr"] = {"in": "roll", "beat": beat, "key": int(ptr["key"])}
    sc = x.get("scroll")
    if isinstance(sc, dict) and finite(sc.get("beat"), lo=-1e3, hi=1e6) is not None:
        out["scroll"] = {"beat": float(sc["beat"]), "row": finite(sc.get("row"), 0.0, 0, 1e4)}
    roll = x.get("roll")
    if isinstance(roll, dict) and finite(roll.get("beat"), lo=-1e3, hi=1e6) is not None:
        out["roll"] = {"beat": float(roll["beat"]), "key": finite(roll.get("key"), 60.0, 0, 127)}
    return out


def file_name(name: str) -> str:
    """A song or track name as part of a file name, on any system."""
    out = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "_", str(name)).strip(" .")
    return out[:80] or "Untitled"


def clean_inst(x: dict | None) -> dict | None:
    """An instrument as the song keeps it: its kind, knobs, pads and plugin."""
    if not x:
        return None
    out = {k: v for k, v in x.items() if k not in ("error", "pad_names")}
    if isinstance(out.get("vst"), dict):
        out["vst"] = {k: out["vst"].get(k) for k in ("path", "cid", "name", "state")}
    return out


def clean_slot(x: dict) -> dict:
    """A rack slot as the project keeps it: settings only, nothing this machine found out."""
    out = {k: v for k, v in x.items() if k not in ("error", "model_name", "model_sr", "includes_cab")}
    if isinstance(out.get("vst"), dict):
        out["vst"] = {k: out["vst"].get(k) for k in ("path", "cid", "name", "state")}
    return out
