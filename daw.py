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
tempo, like the grid); offsets into audio and clip lengths are in seconds.
"""
from __future__ import annotations

import base64
import hashlib
import json
import os
import random
import re
import shutil
import string
import threading
import time
import traceback
from pathlib import Path

KINDS = ("meta", "track", "clip", "file")
MAX_TRACKS = 32
META_DEFAULTS = {"name": "Untitled", "bpm": 120.0, "beats": 4, "loop": False, "loop_start": 0.0, "loop_end": 16.0}
# per field, what the UI may set (anything else in an edit is ignored)
FIELDS = {
    "meta": {"name", "bpm", "beats", "loop", "loop_start", "loop_end"},
    "track": {"name", "order", "owner", "src", "arm", "vol", "pan", "rack", "del"},
    "clip": {"track", "file", "start", "offset", "len", "gain", "fade_in", "fade_out", "del"},
    "file": {"frames", "sr", "owner", "del"},
}


# ids and file names end up in paths: only plain names, whatever the partner sends
SAFE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,80}$")


def safe(name) -> bool:
    return isinstance(name, str) and bool(SAFE.match(name)) and ".." not in name


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
        self.sent_desc = None
        self.sent_loop = None
        self.sent_click = None
        self.cursor = 0.0  # playhead while stopped, in beats
        self.play_from = 0.0
        self.rec: dict | None = None
        self.last_sum = 0.0
        self.mismatches = 0
        self.last_snap = 0.0
        self.last_files = 0.0
        self.last_hello = 0.0
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
        self.cursor = 0.0
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
            ops = []
            for ch in changes:
                kind, oid, fields = ch
                if kind not in KINDS:
                    continue
                fields = {f: v for f, v in fields.items() if f in FIELDS[kind]}
                if fields:
                    ops.append(self.p.make_op(kind, str(oid), fields, self.me))
            self._apply(ops, local=True)
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
            rack = [clean_slot(x) for x in self.e.mixer()["tracks"][slot]["rack"]]
            self.sent_rack[slot] = rack
            self._apply([self.p.make_op("track", tid, {"rack": rack}, self.me)], local=True)

    def sync_rack_states(self, tracks: list):
        """Plugin settings changed in a plugin's own window don't pass through
        Duet: with the freshly read mixer, put any that changed into the song."""
        with self.lock:
            ops = []
            for tid, slot in self.slots.items():
                rack = [clean_slot(x) for x in tracks[slot]["rack"]]
                if any(x.get("vst") for x in rack) and rack != self.p.live("track").get(tid, {}).get("rack"):
                    self.sent_rack[slot] = rack
                    ops.append(self.p.make_op("track", tid, {"rack": rack}, self.me))
            if ops:
                self._apply(ops, local=True)

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
            mine = {tid: t for tid, t in self.p.live("track").items()
                    if t.get("arm") and t.get("owner") == self.me and tid in self.slots}
            for tid, t in mine.items():
                src = int(t.get("src", 0))
                if src not in sources:
                    sources[src] = new_id("take-") + ".wav"
            self.p.audio_dir.mkdir(parents=True, exist_ok=True)
            self.rec = {"punch": at, "sources": sources, "tracks": {tid: int(t.get("src", 0)) for tid, t in mine.items()}}
        if sources:
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
        takes = self.e.daw_stop()
        rec, self.rec = self.rec, None
        if rec and takes:
            spb = self._spb()
            ops = []
            for take in takes:
                if take["error"] or not take["frames"]:
                    if take["error"]:
                        self.notices.append("Recording failed: " + take["error"])
                    continue
                key = take["key"]
                self.peak_cache[key] = base64.b64encode(take["peaks"]).decode()
                self.loaded.add(key)
                ops.append(self.p.make_op("file", key, {"frames": take["frames"], "sr": take["sr"], "owner": self.me}, self.me))
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
                        ops.append(self.p.make_op("clip", new_id("c"), {
                            "track": tid, "file": key, "start": start / spb, "offset": offset, "len": length,
                            "gain": 1.0, "fade_in": 0.0, "fade_out": 0.0}, self.me))
                if take["dropped"]:
                    self.notices.append("The recording skipped a little (the disk couldn't keep up).")
            self._apply(ops, local=True)
            self._flush()
        self.cursor = self.play_from  # back to where you started, like FL
        self.e.daw_locate(self.cursor * self._spb())
        if broadcast:
            self._send({"t": "tp", "play": False})
        self.version += 1

    def locate(self, pos: float):
        with self.lock:
            self.cursor = max(-64.0, float(pos))
            if self._playing():
                self.e.daw_locate(self.cursor * self._spb())
            self.version += 1

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
        byid = dict(tracks)
        for tid, slot in self.slots.items():
            t = byid[tid]
            want = {"active": 1, "gain": 1.0, "fader": float(t.get("vol", 0.8)), "pan": float(t.get("pan", 0.0)),
                    "mute": float(bool(p.local["mute"].get(tid))), "solo": float(bool(p.local["solo"].get(tid)))}
            for k, v in want.items():
                if self.sent_strip.get((slot, k)) != v:
                    e.strip_set(f"T{slot}", k, v)
                    self.sent_strip[(slot, k)] = v
            rack = t.get("rack") or []
            if self.sent_rack.get(slot) != rack:
                self._apply_rack(slot, self.sent_rack.get(slot) or [], rack)
                self.sent_rack[slot] = json.loads(json.dumps(rack))
        spb = self._spb()
        desc = [[] for _ in range(MAX_TRACKS)]
        for c in p.live("clip").values():
            slot = self.slots.get(c.get("track"))
            key = c.get("file")
            if slot is None or key not in self.loaded:
                continue
            desc[slot].append((key, float(c.get("start", 0)) * spb, float(c.get("offset", 0)), float(c.get("len", 0)),
                               float(c.get("gain", 1.0)), float(c.get("fade_in", 0)), float(c.get("fade_out", 0))))
        if desc != self.sent_desc:
            e.daw_timeline(desc)
            self.sent_desc = desc
        m = p.meta()
        loop = (bool(m["loop"]), float(m["loop_start"]) * spb, float(m["loop_end"]) * spb)
        if loop != self.sent_loop:
            e.daw_loop(*loop)
            self.sent_loop = loop
        click = (bool(p.local.get("click")), float(m["bpm"]), int(m["beats"]))
        if click != self.sent_click:
            e.daw_click(*click)
            self.sent_click = click

    def _apply_rack(self, slot: int, old: list, new: list):
        t = f"T{slot}"
        same_shape = len(old) == len(new) and all(
            a.get("kind") == b.get("kind") and a.get("model") == b.get("model")
            and (a.get("vst") or {}).get("cid") == (b.get("vst") or {}).get("cid")
            for a, b in zip(old, new))
        if not same_shape:
            try:
                self.e.rack_load(t, new)
            except Exception as exc:  # a plugin the other side has and we don't
                self.notices.append(f"Couldn't load an effect on a track: {exc}")
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
        elif t == "sum" and pid == self.p.pid:
            if m.get("d") != self.p.digest():
                self.mismatches += 1
                if self.mismatches >= 2 and time.monotonic() - self.last_snap > 4:
                    self.mismatches = 0
                    self.last_snap = time.monotonic()
                    self._send({"t": "snap", "adopt": False, "ops": self.p.all_ops()})
            else:
                self.mismatches = 0

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
            files = {}
            for name, f in p.live("file").items():
                files[name] = {**f, "here": name in self.loaded, "mine": f.get("owner") == self.me}
            return {
                "pid": p.pid, "meta": p.meta(), "tracks": tracks,
                "clips": [{"id": i, **c} for i, c in p.live("clip").items()],
                "files": files, "me": self.me, "partner": self.partner,
                "local": {"click": p.local.get("click"), "countin": p.local.get("countin")},
                "conflict": self.conflict, "shared": self.connected and self.partner_pid == p.pid,
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
            return {"v": self.version, "pos": s["daw_pos"] / spb if playing else self.cursor, "playing": playing,
                    "recording": bool(s["daw_recording"]) or bool(self.rec and playing),
                    "recFrom": self.rec["punch"] / spb if self.rec else None,
                    "levels": levels, "xfer": xfer, "notices": notices}

    def close(self):
        self._running = False
        with self.lock:
            if self.rec:
                self._stop(broadcast=False)
            try:
                self.p.save()
            except OSError:
                pass


def clean_slot(x: dict) -> dict:
    """A rack slot as the project keeps it: settings only, nothing this machine found out."""
    out = {k: v for k, v in x.items() if k not in ("error", "model_name", "model_sr", "includes_cab")}
    if isinstance(out.get("vst"), dict):
        out["vst"] = {k: out["vst"].get(k) for k in ("path", "cid", "name", "state")}
    return out
