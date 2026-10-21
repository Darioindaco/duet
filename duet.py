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
import json
import os
import random
import socket
import subprocess
import sys
import threading
import webbrowser
from pathlib import Path

import webview

from duet_engine import Engine
from t3k import Tone3000
from vstlib import VstLibrary, scan_main


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
        self._window = None  # set once the window exists (for file dialogs)
        self._t3k = Tone3000(data_dir() / "models", lambda: load_settings().get("t3k_key"))
        self._vst = VstLibrary(data_dir() / "vst_cache.json")
        self._names = {}
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
            "metro": {"beat": s["metro_beat"], "synced": s["metro_synced"]},
            "stats": {k: s[k] for k in ("depth", "underruns", "lost", "late", "trimmed", "xruns")},
            "cpu": s["cpu_us"],
            "dspLoad": s["cpu_us"] / (frame / sr * 1e6) if frame else 0.0,
            "warning": s["warning"],
            "signals": self._engine.drain_signals(),
        }

    # --- shared metronome -----------------------------------------------------

    def metro_lead(self, on, bpm, beats):
        return self._engine.metro_lead(bool(on), float(bpm), int(beats))

    def metro_follow(self, on, bpm, beats, epoch):
        self._engine.metro_follow(bool(on), float(bpm), int(beats), int(epoch))

    def metro_volume(self, volume):
        self._engine.metro_volume(float(volume))

    # --- NAM amps and TONE3000 ---------------------------------------------

    def set_model(self, target, pos, path):
        """Load a .nam into a NAM slot. Returns the mixer, or {"error": ...}."""
        try:
            self._engine.rack_set_model(target, int(pos), path)
        except Exception as exc:
            return {"error": str(exc)}
        if path:
            self._t3k.remember_file(path)
        return self._engine.mixer()

    def nam_library(self):
        return self._t3k.library()

    def nam_open_file(self):
        """Native file picker for a .nam; returns the path or None."""
        if not self._window:
            return None
        picked = self._window.create_file_dialog(webview.FileDialog.OPEN, allow_multiple=False,
                                                 file_types=("NAM models (*.nam)", "All files (*.*)"))
        return picked[0] if picked else None

    def t3k_has_key(self):
        return bool(load_settings().get("t3k_key"))

    def t3k_set_key(self, key):
        key = (key or "").strip()
        if not key.startswith("t3k_pub_"):
            return {"error": "That isn't a publishable key. It starts with t3k_pub_ (not t3k_cs_, keep that one secret)."}
        settings = load_settings()
        settings["t3k_key"] = key
        save_settings(settings)
        return {"ok": True}

    def t3k_pick(self):
        return self._t3k.start_pick()

    def t3k_cancel(self):
        self._t3k.cancel()

    def t3k_status(self):
        return self._t3k.status

    def t3k_download(self, model_id):
        try:
            return self._t3k.download(model_id)
        except Exception as exc:
            return {"error": str(exc)}

    # --- VST3 plugins --------------------------------------------------------

    def vst_scan(self, force=False):
        return self._vst.scan(bool(force))

    def vst_status(self):
        return self._vst.status

    def vst_plugins(self):
        return {"plugins": self._vst.plugins(), "failed": self._vst.failed(), "status": self._vst.status}

    def vst_add(self, target, path, cid, name):
        """Load a plugin into a rack. Returns the mixer, or {"error": ...}."""
        try:
            self._engine.rack_add_vst(target, path, cid, name)
        except Exception as exc:
            return {"error": str(exc)}
        return self._engine.mixer()

    def vst_open(self, target, pos):
        try:
            self._engine.vst_open_editor(target, int(pos))
            return {"ok": True}
        except Exception as exc:
            return {"error": str(exc)}

    # --- mixer persistence: a file, so plugin settings changed in their own
    #     windows are saved too (on every change and when Duet closes)

    def save_mixer(self, names=None):
        if names is not None:
            self._names = names
        snap = {"mixer": self._engine.mixer(True), "names": self._names}
        try:
            (data_dir() / "mixer.json").write_text(json.dumps(snap), encoding="utf-8")
        except OSError:
            pass
        return True

    def saved_mixer(self):
        try:
            return json.loads((data_dir() / "mixer.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None

    def open_url(self, url):
        if str(url).startswith("https://"):
            webbrowser.open(url)

    # --- video call signaling (WebRTC runs in the page; we only carry its messages)

    def send_signal(self, text):
        self._engine.send_signal(text)

    def shutdown(self):
        self.save_mixer()  # plugin windows may have changed settings since the last save
        self._engine.close()


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--vst-scan":
        scan_main(sys.argv[2])  # child process of the plugin scanner; exits
    if sys.platform == "darwin":
        # plugin windows must live on the main thread, which pywebview runs
        os.environ["DUET_VST_MAIN_THREAD"] = "1"
    ap = argparse.ArgumentParser(description="Duet: peer-to-peer jamming")
    ap.add_argument("--port", type=int, default=0, help="fixed UDP port (default: the one from last time)")
    ap.add_argument("--debug", action="store_true", help="open the webview dev tools")
    args = ap.parse_args()
    engine = open_engine(args.port)
    engine.start()
    api = Api(engine)
    window = webview.create_window("Duet", url=str(resource("ui/index.html")), js_api=api,
                                   width=1280, height=900, min_size=(980, 780),
                                   background_color="#0e0d12")
    api._window = window
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


def load_settings() -> dict:
    try:
        return json.loads((data_dir() / "settings.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}


def save_settings(settings: dict):
    try:
        (data_dir() / "settings.json").write_text(json.dumps(settings, indent=1), encoding="utf-8")
    except OSError:
        pass


def open_engine(port: int = 0) -> Engine:
    """Same UDP port every launch, so your code (public ip:port) usually stays the
    same too and both sides can simply reconnect to the code they saved. A random
    high port the first time; any port if that one is taken now."""
    settings = load_settings()
    port = port or settings.get("udp_port") or random.randint(49152, 64999)
    try:
        engine = Engine(port=port)
    except Exception:
        engine = Engine(port=0)
    settings["udp_port"] = engine.local_port
    save_settings(settings)
    return engine


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
