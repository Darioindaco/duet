"""Updating Duet by itself, from the releases on GitHub.

A build made by CI carries build_info.json ({"run": the CI run number, "sha", "ref"}); a release's
tag ends in the run number of the build in it (build-20261103-110), so a higher number is newer.
check() looks for the newest release with a download for this computer (the Apple Silicon or Intel
Mac app, or the Windows installer: a release may lack one, e.g. Intel only comes when asked for),
download() fetches it in the background, install() puts it in place and starts it again:

- Mac: the new Duet.app replaces this one where it is (a little script waits for Duet to quit,
  swaps the two, clears the "downloaded from the internet" flag and opens it).
- Windows: the installer runs without asking anything (into the same folder) and starts Duet.

Something the app downloads itself isn't marked as coming from the internet, so neither macOS's
"unidentified developer" nor SmartScreen's warning comes back after an update.
A copy run from source (no build_info.json) never updates itself.
"""
from __future__ import annotations

import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import urllib.request
from pathlib import Path

REPO = "Darioindaco/duet"
RELEASES = f"https://api.github.com/repos/{REPO}/releases?per_page=20"


def run_of(tag: str) -> int | None:
    m = re.fullmatch(r"build-\d{8}-(\d+)", str(tag or ""))
    return int(m.group(1)) if m else None


def asset_name() -> str | None:
    """The download this computer needs (None: no updates here)."""
    if sys.platform == "win32":
        return "Duet-setup.exe"
    if sys.platform == "darwin":
        return "Duet-mac-apple-silicon.zip" if platform.machine() == "arm64" else "Duet-mac-intel.zip"
    return None


def newest(releases: list, name: str) -> dict | None:
    """The release with the highest run number that has `name` among its files."""
    best = None
    for r in releases:
        if r.get("draft") or r.get("prerelease"):
            continue
        run = run_of(r.get("tag_name"))
        asset = next((a for a in r.get("assets") or [] if a.get("name") == name and a.get("state", "uploaded") == "uploaded"), None)
        if run is None or not asset:
            continue
        if best is None or run > best["run"]:
            best = {"run": run, "tag": r["tag_name"], "url": asset["browser_download_url"], "size": int(asset.get("size") or 0),
                    "notes": (r.get("body") or "")[:600], "page": r.get("html_url"), "date": r.get("published_at")}
    return best


def mac_app_path() -> Path | None:
    """This Duet.app (from inside it: .../Duet.app/Contents/MacOS/Duet)."""
    p = Path(sys.executable).resolve()
    for parent in p.parents:
        if parent.suffix == ".app":
            return parent
    return None


class Updater:
    def __init__(self, info: dict | None, work_dir: Path, https_context=None):
        self.info = info or {}
        self.dir = work_dir
        self.ctx = https_context
        self.lock = threading.Lock()
        self.found: dict | None = None
        self.state = {"state": "idle", "done": 0, "total": 0, "error": None}
        self.file: Path | None = None

    # --- what's there ---------------------------------------------------------

    def current(self) -> int | None:
        try:
            return int(self.info.get("run"))
        except (TypeError, ValueError):
            return None

    def supported(self) -> str | None:
        """Why this copy can't update itself (None: it can)."""
        if self.current() is None:
            return "This copy wasn't built by CI (running from source), so it doesn't update itself."
        if not asset_name():
            return "Updates are for the Mac and Windows apps."
        if sys.platform == "darwin":
            app = mac_app_path()
            if not app:
                return "Can't find Duet.app."
            if "AppTranslocation" in str(app):
                return "Move Duet to your Applications folder first (macOS runs it from a read-only copy where it is now)."
            if not os.access(app.parent, os.W_OK):
                return f"Duet can't write to {app.parent}: move it to your Applications folder."
        return None

    def check(self, fetch=None) -> dict:
        why = self.supported()
        out = {"current": self.current(), "sha": self.info.get("sha"), "supported": why is None, "why": why}
        if why:
            return out
        try:
            if fetch is None:
                req = urllib.request.Request(RELEASES, headers={"User-Agent": "Duet-updater", "Accept": "application/vnd.github+json"})
                with urllib.request.urlopen(req, timeout=15, context=self.ctx) as r:
                    releases = json.loads(r.read().decode("utf-8"))
            else:
                releases = fetch()
        except Exception as exc:  # offline, rate limited: try again later
            return {**out, "error": f"Couldn't look for updates: {exc}"}
        best = newest(releases, asset_name())
        with self.lock:
            self.found = best if best and best["run"] > (self.current() or 0) else None
        if not self.found:
            return {**out, "available": False, "latest": best and best["run"]}
        return {**out, "available": True, "latest": self.found["run"], "tag": self.found["tag"], "size": self.found["size"],
                "page": self.found["page"], "date": self.found["date"]}

    # --- getting it -----------------------------------------------------------

    def status(self) -> dict:
        with self.lock:
            return dict(self.state)

    def download(self) -> dict:
        with self.lock:
            if not self.found:
                return {"error": "There's no update to get."}
            if self.state["state"] in ("downloading", "ready"):
                return {"ok": True}
            self.state = {"state": "downloading", "done": 0, "total": self.found["size"], "error": None}
            found = dict(self.found)
        threading.Thread(target=self._download, args=(found,), name="duet-update", daemon=True).start()
        return {"ok": True}

    def _download(self, found: dict, opener=None):
        name = asset_name()
        self.dir.mkdir(parents=True, exist_ok=True)
        dest = self.dir / f"{found['run']}-{name}"
        part = dest.with_suffix(dest.suffix + ".part")
        try:
            if not (dest.exists() and found["size"] and dest.stat().st_size == found["size"]):
                req = urllib.request.Request(found["url"], headers={"User-Agent": "Duet-updater"})
                with (opener or urllib.request.urlopen)(req, timeout=60, context=self.ctx) as r, open(part, "wb") as f:
                    total = int(r.headers.get("Content-Length") or found["size"] or 0)
                    done = 0
                    while chunk := r.read(1 << 16):
                        f.write(chunk)
                        done += len(chunk)
                        with self.lock:
                            self.state.update(done=done, total=total)
                if found["size"] and part.stat().st_size != found["size"]:
                    raise OSError(f"the download is {part.stat().st_size} bytes, not {found['size']}")
                os.replace(part, dest)
            # older downloads go
            for old in self.dir.iterdir():
                if old not in (dest, part) and (old.name.endswith(name) or old.suffix == ".part"):
                    shutil.rmtree(old, ignore_errors=True) if old.is_dir() else old.unlink(missing_ok=True)
            with self.lock:
                self.file = dest
                self.state.update(state="ready", done=dest.stat().st_size, total=dest.stat().st_size)
        except Exception as exc:
            part.unlink(missing_ok=True)
            with self.lock:
                self.state.update(state="error", error=f"The download didn't work: {exc}")

    # --- putting it in place ----------------------------------------------------

    def install(self) -> dict:
        """Start putting the new version in place; the caller quits Duet right after (the new one
        starts by itself once this one is gone)."""
        with self.lock:
            f = self.file
        if not f or not f.exists():
            return {"error": "Download the update first."}
        try:
            if sys.platform == "win32":
                flags = getattr(subprocess, "DETACHED_PROCESS", 0) | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
                subprocess.Popen([str(f), "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"],
                                 creationflags=flags, close_fds=True)
                return {"ok": True}
            if sys.platform == "darwin":
                return self._install_mac(f)
        except Exception as exc:
            return {"error": f"Couldn't install the update: {exc}"}
        return {"error": "Updates are for the Mac and Windows apps."}

    def _install_mac(self, zip_path: Path) -> dict:
        app = mac_app_path()
        if not app:
            return {"error": "Can't find Duet.app."}
        # unpacked next to the app, so the swap is a rename on the same disk
        stage = Path(tempfile.mkdtemp(prefix=".Duet-update-", dir=app.parent))
        subprocess.run(["ditto", "-x", "-k", str(zip_path), str(stage)], check=True, timeout=300)
        new = stage / "Duet.app"
        if not (new / "Contents" / "MacOS" / "Duet").exists():
            shutil.rmtree(stage, ignore_errors=True)
            return {"error": "The download doesn't hold a Duet.app."}
        script = stage / "swap.sh"
        script.write_text(MAC_SWAP, encoding="utf-8")
        log = self.dir / "update.log"
        subprocess.Popen(["/bin/bash", str(script), str(os.getpid()), str(app), str(new), str(stage)],
                         stdout=open(log, "w"), stderr=subprocess.STDOUT, start_new_session=True, close_fds=True)
        return {"ok": True}


# waits for Duet to quit, swaps the apps (the old one comes back if anything goes wrong), opens it
MAC_SWAP = r"""#!/bin/bash
pid="$1"; app="$2"; new="$3"; stage="$4"
for _ in $(seq 1 600); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
old="$stage/Duet-old.app"
if mv "$app" "$old"; then
  if mv "$new" "$app"; then rm -rf "$old"; else mv "$old" "$app"; fi
fi
xattr -dr com.apple.quarantine "$app" 2>/dev/null
open "$app"
rm -rf "$stage"
"""


def build_info(path: Path) -> dict | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
