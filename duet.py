"""Duet: free peer-to-peer low-latency jamming for two people.

    python duet.py                 # normal use
    python duet.py --port 50555    # fixed UDP port (if you forward it on your router)

The window is an HTML page (ui/index.html) in a native webview (Edge WebView2 on
Windows, WebKit on macOS). The page calls into the `Api` class below. Audio and
networking live in the Rust engine (engine/, imported as duet_engine) on native
threads that never wait for Python or the UI.
"""
from __future__ import annotations

import argparse
import os
import socket
import subprocess
import sys
import threading
from pathlib import Path

import webview

from duet_engine import Engine


def resource(path: str) -> Path:
    """Locate bundled files both from source and inside a PyInstaller build."""
    base = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parent))
    return base / path


class Api:
    """Everything the page can call, as window.pywebview.api.<name>(...)."""

    def __init__(self, engine: Engine):
        self._engine = engine
        self._code = ""
        self._code_note = ""
        self._audio = False
        threading.Thread(target=self._discover, daemon=True).start()

    def _discover(self):
        self._code = self._engine.discover()
        if not self._engine.public_addr:
            self._code_note = "No internet lookup: this code only works on the same Wi-Fi."

    # --- setup ---------------------------------------------------------------

    def init(self):
        inputs, outputs, din, dout = self._engine.devices()
        return {
            "inputs": inputs, "outputs": outputs,
            "defaultInput": din, "defaultOutput": dout,
            "presets": Engine.presets(),
            "effects": Engine.effects(),
            "platform": sys.platform,
        }

    def start_audio(self, cfg):
        try:
            lin, lout, n_in = self._engine.start_audio(str(cfg["input"]), str(cfg["output"]),
                                                       int(cfg["samplerate"]), int(cfg["block"]))
        except Exception as exc:
            self._audio = False
            return {"ok": False, "error": str(exc)}
        self._audio = True
        return {"ok": True, "latencyIn": lin, "latencyOut": lout, "inChannels": n_in}

    def stop_audio(self):
        self._engine.stop_audio()
        self._audio = False
        return True

    def connect(self, code):
        try:
            self._engine.connect(code)
        except Exception:
            return {"ok": False, "error": "That code doesn't look right. Copy it again with the Copy button."}
        return {"ok": True}

    def copy(self, text):
        cmd = ["pbcopy"] if sys.platform == "darwin" else ["clip"] if sys.platform == "win32" else ["xclip", "-sel", "c"]
        try:
            # clip.exe reads the console codepage; the code is pure ASCII so that's fine
            subprocess.run(cmd, input=text.encode(), check=True,
                           creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
            return True
        except Exception:
            return False

    # --- mixer (strips "L0".."L3" = mine, "R0".."R7" = the partner's) -------

    def mixer(self):
        return self._engine.mixer()

    def strip_set(self, target, key, value):
        self._engine.strip_set(target, key, float(value))

    def set_master(self, value):
        self._engine.set_master(float(value))

    def set_buffer(self, blocks):
        self._engine.set_buffer(float(blocks))

    def rack_add(self, target, kind):
        self._engine.rack_add(target, kind)
        return self._engine.mixer()

    def rack_remove(self, target, pos):
        self._engine.rack_remove(target, int(pos))
        return self._engine.mixer()

    def rack_move(self, target, frm, to):
        self._engine.rack_move(target, int(frm), int(to))
        return self._engine.mixer()

    def rack_set(self, target, pos, key, value):
        self._engine.rack_set(target, int(pos), key, float(value))

    def rack_preset(self, target, name):
        self._engine.rack_preset(target, name)
        return self._engine.mixer()

    def rack_load(self, target, slots):
        self._engine.rack_load(target, list(slots))

    def load_mixer(self, cfg):
        """Restore a mixer() snapshot from an earlier session."""
        e = self._engine
        keys = ("input", "gain", "pan", "fader", "mute", "solo", "send", "mon", "active")
        for side, prefix in (("local", "L"), ("remote", "R")):
            for i, strip in enumerate(cfg.get(side, [])[:4 if side == "local" else 8]):
                t = f"{prefix}{i}"
                for k in keys:
                    if k in strip:
                        e.strip_set(t, k, float(strip[k]))
                e.rack_load(t, list(strip.get("rack", [])))
        if "master" in cfg:
            e.set_master(float(cfg["master"]))
        return True

    # --- polled ~12x per second by the page ----------------------------------

    def state(self):
        s = self._engine.state()
        sr, frame = s["samplerate"], s["frame"]
        return {
            "code": self._code, "codeNote": self._code_note,
            "audio": self._audio,
            "connected": s["connected"],
            "calling": s["calling"],
            "peer": s["peer"],
            "rtt": s["rtt"],
            "latency": {"network": s["rtt"] / 2 if s["rtt"] is not None else None,
                        "buffer": s["jitter_target"] * frame / sr * 1000,
                        "card": (s["lat_in_ms"] + s["lat_out_ms"]) if self._audio else 0.0,
                        "block": frame / sr * 1000},
            "levels": {"local": s["local_levels"], "remote": s["remote_levels"], "master": s["master_level"]},
            "remoteChannels": s["remote_channels"],
            "stats": {k: s[k] for k in ("depth", "underruns", "lost", "late", "trimmed", "xruns")},
            "cpu": s["cpu_us"],
            "warning": s["warning"],
            "signals": self._engine.drain_signals(),
        }

    # --- video call signaling (WebRTC runs in the page; we only carry its messages)

    def send_signal(self, text):
        self._engine.send_signal(text)

    def shutdown(self):
        self._engine.close()


def main():
    ap = argparse.ArgumentParser(description="Duet: peer-to-peer jamming")
    ap.add_argument("--port", type=int, default=0, help="fixed UDP port (default: random)")
    ap.add_argument("--debug", action="store_true", help="open the webview dev tools")
    args = ap.parse_args()
    engine = Engine(port=args.port)
    engine.start()
    api = Api(engine)
    window = webview.create_window("Duet", url=str(resource("ui/index.html")), js_api=api,
                                   width=1280, height=900, min_size=(980, 780),
                                   background_color="#0e0d12")
    window.events.closed += api.shutdown
    # Served from http://127.0.0.1:<fixed port> rather than file://: the camera needs a
    # secure origin, and a stable origin + persistent storage keep saved settings and
    # the camera permission across launches.
    webview.start(debug=args.debug, http_server=True, http_port=pick_port(),
                  private_mode=False, storage_path=str(data_dir() / "webview"))


def data_dir() -> Path:
    if sys.platform == "win32":
        base = Path(os.environ.get("APPDATA", Path.home()))
    elif sys.platform == "darwin":
        base = Path.home() / "Library" / "Application Support"
    else:
        base = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
    path = base / "Duet"
    path.mkdir(parents=True, exist_ok=True)
    return path


def pick_port(preferred: int = 47321) -> int:
    """The same port every launch when possible (same origin = same saved settings)."""
    for port in (preferred, preferred + 1, preferred + 2, 0):
        with socket.socket() as s:
            try:
                s.bind(("127.0.0.1", port))
                return s.getsockname()[1]
            except OSError:
                continue
    return 0


if __name__ == "__main__":
    main()
