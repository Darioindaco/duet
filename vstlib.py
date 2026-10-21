"""The VST3 plugins on this machine: found, scanned and cached.

Scanning a plugin without a moduleinfo.json means loading its code. That
happens in a child process per plugin (`Duet --vst-scan <path>`), with a time
limit, so a plugin that crashes, hangs or pops a license dialog during the scan
can't take Duet with it, and nothing stays loaded in the app. Results are
cached by path + modification time, so later scans are instant.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from duet_engine import Engine

SCAN_TIMEOUT = 30


def scan_command(path: str) -> list:
    if getattr(sys, "frozen", False):
        return [sys.executable, "--vst-scan", path]
    return [sys.executable, str(Path(__file__).with_name("duet.py")), "--vst-scan", path]


def kill_tree(p: subprocess.Popen):
    """The scanner runs behind a launcher (venv python.exe, or the packaged
    app's bootloader), so killing just the process we started would orphan the
    one actually stuck in a plugin."""
    if sys.platform == "win32":
        subprocess.run(["taskkill", "/T", "/F", "/PID", str(p.pid)], capture_output=True,
                       creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    else:
        p.kill()


def scan_one(path: str, timeout: float = SCAN_TIMEOUT) -> dict:
    """In a child process: {"classes": [...]} or {"error": "..."}. Takes the
    answer as soon as it's printed and kills the child: some plugins hang in
    their own shutdown code (seen with Native Instruments' Qt-based ones)."""
    try:
        p = subprocess.Popen(scan_command(path), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                             creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    except Exception as exc:  # noqa: BLE001 - reported per plugin
        return {"error": str(exc)}
    result = {}

    def read():
        for line in p.stdout:
            if line.startswith("{"):
                result["line"] = line
                return
    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    reader.join(timeout)
    if p.poll() is None:
        kill_tree(p)
    if "line" not in result:
        return {"error": "took too long to load (license dialog?)" if reader.is_alive() else f"scanner crashed ({p.poll()})"}
    try:
        return json.loads(result["line"])
    except ValueError:
        return {"error": "scanner said something odd"}


def scan_main(path: str):
    """Entry point of the child process."""
    try:
        print(json.dumps({"classes": Engine.vst_scan(path)}))
    except Exception as exc:  # noqa: BLE001
        print(json.dumps({"error": str(exc)}))
    sys.stdout.flush()
    # Leave without running anyone's shutdown code: os._exit still runs every
    # DLL's detach routine on Windows, and some plugins hang in theirs.
    if sys.platform == "win32":
        import ctypes
        ctypes.windll.kernel32.TerminateProcess(ctypes.windll.kernel32.GetCurrentProcess(), 0)
    os._exit(0)


class VstLibrary:
    def __init__(self, cache_file: Path):
        self.cache_file = cache_file
        self._lock = threading.Lock()
        self.status = {"scanning": False, "done": 0, "total": 0}
        try:
            self._cache = json.loads(cache_file.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            self._cache = {}

    def _save(self):
        try:
            self.cache_file.write_text(json.dumps(self._cache, indent=1), encoding="utf-8")
        except OSError:
            pass

    def scan(self, force: bool = False):
        """Find and scan in the background; poll `status` and `plugins()`."""
        with self._lock:
            if self.status["scanning"]:
                return self.status
            self.status = {"scanning": True, "done": 0, "total": 0}
        threading.Thread(target=self._scan, args=(force,), daemon=True).start()
        return self.status

    def _scan(self, force):
        try:
            paths = Engine.vst_find(Engine.vst_dirs())
            todo = []
            for p in paths:
                try:
                    mtime = os.path.getmtime(p)
                except OSError:
                    continue
                hit = self._cache.get(p)
                if force or not hit or hit.get("mtime") != mtime:
                    todo.append((p, mtime))
            for gone in set(self._cache) - set(paths):
                del self._cache[gone]
            self.status = {"scanning": True, "done": 0, "total": len(todo)}

            def one(item):
                p, mtime = item
                res = scan_one(p)
                with self._lock:
                    self._cache[p] = {"mtime": mtime, **res}
                    self.status["done"] += 1
            with ThreadPoolExecutor(max_workers=4) as pool:
                list(pool.map(one, todo))
            # Some vendors' plugins (Native Instruments' do) load one at a time
            # through a background service and time out when scanned in parallel:
            # give those a second, solo try with more patience.
            retry = [(p, m) for p, m in todo if "too long" in self._cache.get(p, {}).get("error", "")]
            for p, mtime in retry:
                res = scan_one(p, timeout=SCAN_TIMEOUT * 3)
                with self._lock:
                    self._cache[p] = {"mtime": mtime, **res}
            self._save()
        finally:
            self.status = {**self.status, "scanning": False}

    def plugins(self) -> list:
        """Effects only (no instruments: Duet's racks have no MIDI)."""
        out = []
        for path, entry in self._cache.items():
            for c in entry.get("classes", []):
                if c.get("instrument"):
                    continue
                out.append({"path": path, "cid": c["cid"], "name": c["name"], "vendor": c.get("vendor") or "",
                            "sub": c.get("sub") or "", "file": Path(path).name})
        out.sort(key=lambda p: (p["name"].lower(), p["vendor"].lower()))
        return out

    def failed(self) -> list:
        return [{"path": p, "error": e["error"]} for p, e in self._cache.items() if e.get("error")]
