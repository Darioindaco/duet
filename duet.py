"""Duet: free peer-to-peer low-latency jamming for two people.

    python duet.py                 # normal use
    python duet.py --port 50555    # fixed UDP port (if you forward it on your router)

The window is an HTML page (ui/index.html) in a native webview (Edge WebView2 on
Windows, WebKit on macOS). The page calls into the `Api` class below; audio and
networking never touch the UI thread.
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

import audio
from effects import PRESETS
from engine import Engine


def resource(path: str) -> Path:
    """Locate bundled files both from source and inside a PyInstaller build."""
    base = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parent))
    return base / path


class Api:
    """Everything the page can call, as window.pywebview.api.<name>(...)."""

    def __init__(self, engine: Engine):
        self._engine = engine
        self._audio = audio.AudioIO(engine)
        self._code = ""
        self._code_note = ""
        threading.Thread(target=self._discover, daemon=True).start()

    def _discover(self):
        self._code = self._engine.discover()
        if not self._engine.public_addr:
            self._code_note = "No internet lookup: this code only works on the same Wi-Fi."

    # --- setup ---------------------------------------------------------------

    def init(self):
        inputs, outputs = audio.list_devices()
        din, dout = audio.default_devices(inputs, outputs)
        return {
            "inputs": inputs, "outputs": outputs,
            "defaultInput": din, "defaultOutput": dout,
            "fx": self._engine.fx.state(),
            "presets": list(PRESETS),
            "platform": sys.platform,
        }

    def start_audio(self, cfg):
        try:
            self._audio.start(int(cfg["input"]), int(cfg["output"]), cfg["channel"],
                              int(cfg["samplerate"]), int(cfg["block"]))
        except Exception as exc:  # PortAudio errors are plain Exceptions with useful text
            self._audio.stop()
            return {"ok": False, "error": str(exc)}
        lin, lout = self._audio.latency_ms
        return {"ok": True, "latencyIn": lin, "latencyOut": lout}

    def stop_audio(self):
        self._audio.stop()
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
        self._engine.fx.set(effect, key, value)

    def fx_state(self):
        return self._engine.fx.state()

    def preset(self, name):
        self._engine.fx.preset(name)
        return self._engine.fx.state()

    def set_mix(self, key, value):
        e = self._engine
        if key == "partner":
            e.remote_gain = float(value)
        elif key == "monitor":
            e.monitor_gain = float(value)
        elif key == "buffer":
            e.jitter.target = int(value)
        elif key == "room":
            e.room.set("reverb", "on", float(value) > 0.001)
            e.room.set("reverb", "mix", float(value))

    # --- polled ~12x per second by the page ----------------------------------

    def state(self):
        e, a, j = self._engine, self._audio, self._engine.jitter
        lin, lout = a.latency_ms
        connected = e.connected
        net_ms = e.rtt_ms / 2 if (connected and e.rtt_ms is not None) else None
        buf_ms = j.target * e.frame / e.samplerate * 1000
        return {
            "code": self._code, "codeNote": self._code_note,
            "audio": a.running,
            "connected": connected,
            "calling": bool(e.candidates) and not connected,
            "peer": e.peer_addr[0] if (connected and e.peer_addr) else None,
            "rtt": e.rtt_ms if connected else None,
            "latency": {"network": net_ms, "buffer": buf_ms,
                        "card": lin + lout, "block": e.frame / e.samplerate * 1000},
            "levels": {"me": e.in_level, "partner": e.out_level},
            "stats": {"depth": j.depth, "underruns": j.underruns, "lost": j.lost,
                      "late": j.late, "trimmed": j.dropped, "xruns": a.xruns},
            "warning": e.warning or e.fx_error,
            "signals": e.drain_signals(),
        }

    # --- video call signaling (WebRTC runs in the page; we only carry its messages)

    def send_signal(self, text):
        self._engine.send_signal(text)

    def shutdown(self):
        self._audio.stop()
        self._engine.close()


def main():
    ap = argparse.ArgumentParser(description="Duet: peer-to-peer jamming")
    ap.add_argument("--port", type=int, default=0, help="fixed UDP port (default: random)")
    ap.add_argument("--debug", action="store_true", help="open the webview dev tools")
    args = ap.parse_args()
    # The audio callback, the network thread and the UI bridge share the GIL; a
    # shorter switch interval (default 5 ms, longer than one block) keeps audio on time.
    sys.setswitchinterval(0.0005)

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
