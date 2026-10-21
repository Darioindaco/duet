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
            "fx": self._engine.fx_state(),
            "presets": Engine.presets(),
            "platform": sys.platform,
        }

    def start_audio(self, cfg):
        channel = None if cfg["channel"] == "mix" else int(cfg["channel"])
        try:
            lin, lout = self._engine.start_audio(str(cfg["input"]), str(cfg["output"]), channel,
                                                 int(cfg["samplerate"]), int(cfg["block"]))
        except Exception as exc:
            self._audio = False
            return {"ok": False, "error": str(exc)}
        self._audio = True
        return {"ok": True, "latencyIn": lin, "latencyOut": lout}

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

    # --- live controls -------------------------------------------------------

    def set_fx(self, effect, key, value):
        self._engine.set_fx(effect, key, float(value))

    def fx_state(self):
        return self._engine.fx_state()

    def preset(self, name):
        return self._engine.preset(name)

    def set_mix(self, key, value):
        self._engine.set_mix(key, float(value))

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
            "levels": {"me": s["in_level"], "partner": s["out_level"]},
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
