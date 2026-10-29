"""The sample library: the browser on the left, like FL Studio's.

Two kinds of places hold sounds:

- Folders on disk. "Duet sounds" (the built-in drum kit as WAV files, made
  the first time), "My samples" (yours: you make folders in it and import
  files into it), and any folder you add (your sample packs where they are).
  These are real folders: what's in them is what's on disk.
- Bins, as in Premiere or Avid: collections you make yourself, that point at
  files wherever they are, and can hold other bins. A file can sit in any
  number of bins; taking it out of one leaves the file alone.

Only this machine's library: it isn't shared. A sample that goes into the
song is copied into the project, and the partner gets it from there.
"""
from __future__ import annotations

import json
import os
import re
import shutil
import time
import uuid
from pathlib import Path

# what Duet reads (the engine decodes all of these)
AUDIO = (".wav", ".wave", ".mp3", ".m4a", ".aac", ".mp4", ".flac", ".aif", ".aiff", ".aifc", ".ogg", ".oga")
NAME = re.compile(r'[<>:"/\\|?*\x00-\x1f]')
MAX_SEARCH = 400


def clean_name(name: str) -> str:
    """A name for a folder or bin: no path tricks, nothing a file system minds."""
    out = NAME.sub("_", str(name)).strip(" .")
    return out[:80] or "New folder"


class Library:
    def __init__(self, root: Path, engine_cls=None):
        self.root = root
        self.root.mkdir(parents=True, exist_ok=True)
        self.mine = root / "My samples"
        self.sounds = root / "Duet sounds"
        self.mine.mkdir(exist_ok=True)
        self.file = root / "library.json"
        self.data = {"folders": [], "bins": {}, "recent": []}
        try:
            loaded = json.loads(self.file.read_text(encoding="utf-8"))
            if isinstance(loaded, dict):
                self.data.update({k: v for k, v in loaded.items() if k in self.data})
        except (OSError, ValueError):
            pass
        if engine_cls is not None:
            self._make_sounds(engine_cls)

    def _save(self):
        try:
            tmp = self.file.with_suffix(".tmp")
            tmp.write_text(json.dumps(self.data, indent=1), encoding="utf-8")
            os.replace(tmp, self.file)
        except OSError:
            pass

    def _make_sounds(self, engine_cls):
        """The built-in kit as WAV files, once."""
        drums = self.sounds / "Drum kit"
        if drums.exists() and any(drums.glob("*.wav")):
            return
        drums.mkdir(parents=True, exist_ok=True)
        for i, (note, name) in enumerate(engine_cls.drum_pads()):
            try:
                engine_cls.drum_sound_wav(i, str(drums / f"{note} {name}.wav"))
            except Exception:
                pass

    # --- places ------------------------------------------------------------------

    def roots(self) -> list[dict]:
        out = [{"path": str(self.sounds), "name": "Duet sounds", "kind": "sounds", "writable": False},
               {"path": str(self.mine), "name": "My samples", "kind": "mine", "writable": True}]
        for p in self.data["folders"]:
            out.append({"path": p, "name": Path(p).name or p, "kind": "folder", "writable": True,
                        "missing": not Path(p).is_dir()})
        return out

    def _inside(self, path) -> Path | None:
        """`path` if it's in one of the library's folders (nothing else is listed or changed)."""
        try:
            p = Path(path).resolve()
        except (OSError, TypeError, ValueError):
            return None
        for r in self.roots():
            base = Path(r["path"]).resolve()
            if p == base or base in p.parents:
                return p
        return None

    def _writable(self, path) -> Path | None:
        p = self._inside(path)
        if p is None:
            return None
        sounds = self.sounds.resolve()
        return None if p == sounds or sounds in p.parents else p

    def listing(self, path) -> dict:
        """The folders and sounds directly in `path`."""
        p = self._inside(path)
        if p is None or not p.is_dir():
            return {"path": str(path), "folders": [], "files": [], "error": "That folder isn't there."}
        folders, files = [], []
        try:
            for e in sorted(p.iterdir(), key=lambda e: e.name.lower()):
                if e.name.startswith("."):
                    continue
                if e.is_dir():
                    folders.append({"name": e.name, "path": str(e)})
                elif e.suffix.lower() in AUDIO:
                    files.append(self._file(e))
        except OSError as exc:
            return {"path": str(p), "folders": [], "files": [], "error": str(exc)}
        return {"path": str(p), "folders": folders, "files": files, "writable": self._writable(p) is not None}

    @staticmethod
    def _file(p: Path) -> dict:
        try:
            size = p.stat().st_size
        except OSError:
            size = 0
        return {"name": p.stem, "path": str(p), "size": size}

    def search(self, query: str) -> list[dict]:
        """Sounds whose name has every word of `query`, in every folder."""
        words = [w for w in str(query).lower().split() if w]
        if not words:
            return []
        out = []
        for r in self.roots():
            base = Path(r["path"])
            if not base.is_dir():
                continue
            for dirpath, dirnames, filenames in os.walk(base):
                dirnames[:] = [d for d in dirnames if not d.startswith(".")]
                for f in filenames:
                    if f.lower().endswith(AUDIO) and all(w in f.lower() for w in words):
                        out.append(self._file(Path(dirpath) / f))
                        if len(out) >= MAX_SEARCH:
                            return out
        return out

    def add_folder(self, path: str) -> dict:
        p = Path(path)
        if not p.is_dir():
            return {"error": "That isn't a folder."}
        s = str(p.resolve())
        if s not in self.data["folders"] and self._inside(s) is None:
            self.data["folders"].append(s)
            self._save()
        return {"ok": True, "path": s}

    def remove_folder(self, path: str):
        """Out of the browser (the folder itself stays on disk)."""
        self.data["folders"] = [f for f in self.data["folders"] if f != path]
        self._save()

    def new_folder(self, parent: str, name: str) -> dict:
        p = self._writable(parent)
        if p is None or not p.is_dir():
            return {"error": "New folders go in My samples or a folder you added."}
        base = clean_name(name)
        dst, n = p / base, 2
        while dst.exists():
            dst, n = p / f"{base} {n}", n + 1
        try:
            dst.mkdir()
        except OSError as exc:
            return {"error": str(exc)}
        return {"ok": True, "path": str(dst)}

    def rename(self, path: str, name: str) -> dict:
        """A folder or sound in My samples or an added folder."""
        p = self._writable(path)
        if p is None or p in [Path(r["path"]).resolve() for r in self.roots()]:
            return {"error": "That one can't be renamed here."}
        dst = p.with_name(clean_name(name) + (p.suffix if p.is_file() else ""))
        if dst.exists():
            return {"error": "There's already one with that name."}
        try:
            p.rename(dst)
        except OSError as exc:
            return {"error": str(exc)}
        for b in self.data["bins"].values():
            b["items"] = [str(dst) if i == str(p) else i for i in b["items"]]
        self._save()
        return {"ok": True, "path": str(dst)}

    def import_files(self, paths: list, dest: str | None = None) -> dict:
        """Copy sounds into My samples (or one of its folders)."""
        d = self._writable(dest) if dest else self.mine
        if d is None or not d.is_dir():
            d = self.mine
        done = []
        for src in paths or []:
            s = Path(src)
            if not s.is_file() or s.suffix.lower() not in AUDIO:
                continue
            dst, n = d / s.name, 2
            while dst.exists():
                dst, n = d / f"{s.stem} {n}{s.suffix}", n + 1
            try:
                shutil.copy2(s, dst)
                done.append(str(dst))
            except OSError:
                pass
        return {"ok": True, "files": done, "folder": str(d)}

    def used(self, path: str):
        """Remember it as recently used (a list in the browser)."""
        r = [p for p in self.data["recent"] if p != path]
        self.data["recent"] = [path] + r[:29]
        self._save()

    def recent(self) -> list[dict]:
        return [self._file(Path(p)) for p in self.data["recent"] if Path(p).is_file()]

    # --- bins --------------------------------------------------------------------

    def bins(self) -> list[dict]:
        out = []
        for bid, b in self.data["bins"].items():
            out.append({"id": bid, "name": b["name"], "parent": b.get("parent"), "count": len(b["items"]),
                        "created": b.get("created", 0)})
        return sorted(out, key=lambda b: (b["created"], b["name"].lower()))

    def new_bin(self, name: str, parent: str | None = None) -> dict:
        if parent is not None and parent not in self.data["bins"]:
            parent = None
        bid = "b" + uuid.uuid4().hex[:10]
        self.data["bins"][bid] = {"name": clean_name(name or "New bin"), "parent": parent, "items": [], "created": time.time()}
        self._save()
        return {"ok": True, "id": bid}

    def rename_bin(self, bid: str, name: str):
        if bid in self.data["bins"]:
            self.data["bins"][bid]["name"] = clean_name(name)
            self._save()

    def delete_bin(self, bid: str):
        """The bin and the bins in it (the sounds themselves stay where they are)."""
        gone = {bid}
        while True:
            more = {k for k, b in self.data["bins"].items() if b.get("parent") in gone} - gone
            if not more:
                break
            gone |= more
        for k in gone:
            self.data["bins"].pop(k, None)
        self._save()

    def move_bin(self, bid: str, parent: str | None):
        """Into another bin (not into itself or one of its own)."""
        b = self.data["bins"].get(bid)
        if not b:
            return
        p = parent
        while p:
            if p == bid:
                return
            p = self.data["bins"].get(p, {}).get("parent")
        b["parent"] = parent if parent in self.data["bins"] else None
        self._save()

    def bin_add(self, bid: str, paths: list) -> dict:
        b = self.data["bins"].get(bid)
        if not b:
            return {"error": "That bin is gone."}
        for p in paths or []:
            p = str(p)
            if p.lower().endswith(AUDIO) and Path(p).is_file() and p not in b["items"]:
                b["items"].append(p)
        self._save()
        return {"ok": True, "count": len(b["items"])}

    def bin_remove(self, bid: str, path: str):
        b = self.data["bins"].get(bid)
        if b:
            b["items"] = [i for i in b["items"] if i != path]
            self._save()

    def bin_listing(self, bid: str) -> dict:
        b = self.data["bins"].get(bid)
        if not b:
            return {"bin": bid, "bins": [], "files": [], "error": "That bin is gone."}
        kids = [x for x in self.bins() if x["parent"] == bid]
        files = [{**self._file(Path(p)), "missing": not Path(p).is_file()} for p in b["items"]]
        return {"bin": bid, "name": b["name"], "bins": kids, "files": files}
