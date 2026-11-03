"""Duet: free peer-to-peer low-latency jamming for two people.

    python duet.py                 # normal use
    python duet.py --port 50555    # fixed UDP port (if you forward it on your router)

The window is an HTML page (ui/index.html) in a native webview (Edge WebView2 on
Windows, WebKit on macOS). The page calls into the `Api` class below. Audio and
networking live in the Rust engine (engine/, imported as duet_engine) on native
threads that never wait for Python or the UI.
"""
from __future__ import annotations

import sys

if __name__ == "__main__" and sys.argv[1:2] == ["--plugin-host"] and len(sys.argv) == 4:
    # the plugin process (plugins in a process of their own, see engine/src/sandbox.rs): nothing of
    # the app is needed, so it starts quickly
    from duet_engine import Engine as _E
    _E.plugin_host(sys.argv[2], sys.argv[3])
    sys.exit(0)

import argparse
import base64
import hashlib
import json
import re
import os
import random
import shutil
import socket
import subprocess
import threading
import webbrowser
from pathlib import Path

import webview

from daw import Daw, file_name
from library import Library
from duet_engine import Engine
from t3k import Tone3000
from updater import Updater, build_info
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
        self._may_close = False  # closing was asked about (or doesn't need to be)
        self._t3k = Tone3000(data_dir() / "models", lambda: load_settings().get("t3k_key"))
        self._vst = VstLibrary(data_dir() / "vst_cache.json")
        self._names = {}
        settings = load_settings()
        if not settings.get("peer_id"):
            settings["peer_id"] = "u" + os.urandom(6).hex()
            save_settings(settings)
        self._lib = Library(data_dir() / "Library", Engine)
        self._daw = Daw(engine, data_dir() / "Projects", settings["peer_id"], float(settings.get("rec_offset_ms", 0)))
        from daw import https_context
        self._upd = Updater(build_info(resource("build_info.json")), data_dir() / "updates", https_context())
        engine.mix_threads(bool(settings.get("mix_threads", True)))
        self._talk_input = settings.get("talk_input")
        if self._talk_input is not None:
            engine.talk_input(int(self._talk_input))
        threading.Thread(target=self._discover, daemon=True).start()

    def _discover(self):
        self._code = self._engine.discover()
        if not self._engine.public_addr:
            self._code_note = "No internet lookup: this code only works on the same Wi-Fi."

    # --- setup ---------------------------------------------------------------

    def init(self):
        inputs, outputs, din, dout, sys_in, sys_out = self._engine.devices()
        return {
            "inputs": inputs, "outputs": outputs,
            "defaultInput": din, "defaultOutput": dout,
            "systemInput": sys_in, "systemOutput": sys_out,
            "presets": Engine.presets(),
            "effects": Engine.effects(),
            "platform": sys.platform,
        }

    def devices_poll(self):
        """Cheap re-read of the device list (no ASIO drivers), every few seconds:
        headphones plugged in or out, and where "System default" points now."""
        inputs, outputs, _din, _dout, sys_in, sys_out = self._engine.devices(False)
        return {"inputs": inputs, "outputs": outputs, "systemInput": sys_in, "systemOutput": sys_out}

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

    def set_buffer_auto(self, on):
        self._engine.set_buffer_auto(bool(on))

    def _rack_changed(self, target):
        # a track's effects, and the master's, are part of the song: the project
        # (and the partner) gets them
        if str(target).startswith("T"):
            self._daw.track_rack_changed(int(target[1:]))
        elif target == "M":
            self._daw.master_rack_changed()

    def rack_add(self, target, kind):
        self._engine.rack_add(target, kind)
        self._rack_changed(target)
        return self._engine.mixer()

    def rack_remove(self, target, pos):
        self._engine.rack_remove(target, int(pos))
        self._rack_changed(target)
        return self._engine.mixer()

    def rack_move(self, target, frm, to):
        self._engine.rack_move(target, int(frm), int(to))
        self._rack_changed(target)
        return self._engine.mixer()

    def midi_controllers(self):
        # the MIDI keyboard's knobs (MIDI-learn): {"count": moves so far, "last": the last one moved,
        # "values": where each is, 0..1 or -1}
        count, last, values = self._engine.midi_controllers()
        return {"count": count, "last": last, "values": values}

    def rack_set(self, target, pos, key, value):
        self._engine.rack_set(target, int(pos), key, float(value))

    def rack_scope(self, target, pos):
        # what an effect's window draws besides its meters (spectra, band levels)
        try:
            return list(self._engine.rack_scope(target, int(pos)))
        except Exception:
            return []

    def rack_meters(self, target):
        # each effect's input, output (peak, 0..1) and gain reduction (dB), for its window
        try:
            return [list(m) for m in self._engine.rack_meters(target)]
        except Exception:
            return []
        self._rack_changed(target)

    def rack_preset(self, target, name):
        self._engine.rack_preset(target, name)
        self._rack_changed(target)
        return self._engine.mixer()

    def rack_load(self, target, slots):
        self._engine.rack_load(target, list(slots))
        self._rack_changed(target)

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
                e.rack_load(t, list(strip.get("rack", [])))  # tracks (T*) come from the song, not from here
                inst = strip.get("inst")
                if side == "local" and inst and inst.get("kind"):
                    try:
                        e.inst_set(t, inst["kind"], None, inst)
                    except Exception:  # a plugin that's gone: back to the input
                        pass
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
                        "bufferBlocks": s["jitter_target"], "bufferAuto": s["jitter_auto"],
                        "card": (s["lat_in_ms"] + s["lat_out_ms"]) if self._audio else 0.0,
                        "block": frame / sr * 1000},
            "levels": {"local": s["local_levels"], "remote": s["remote_levels"], "master": s["master_level"]},
            "remoteChannels": s["remote_channels"],
            "metro": {"beat": s["metro_beat"], "synced": s["metro_synced"]},
            # glitches: the sound card's own (input overflow), the input running
            # dry for the output, input dropped for two clocks drifting apart
            "stats": {**{k: s[k] for k in ("depth", "underruns", "lost", "late", "trimmed")},
                      "xruns": s["xruns"] + s["ring_empty"] + s["in_dropped"]},
            "cpu": s["cpu_us"],
            "dspLoad": s["cpu_us"] / (frame / sr * 1e6) if frame else 0.0,
            "warning": s["warning"],
            "streamError": s["stream_error"],
            "signals": self._daw.take_ui_signals(),
            "midiEvents": s["midi_events"],
            "talk": {"tx": s["talk_tx"], "rx": s["talk_rx"], "input": self._talk_input},
            "daw": self._daw.status(s),
        }

    # --- the song: playlist, transport, recording (daw.py) --------------------

    def daw_freeze(self, track, mode):
        return self._daw.freeze(str(track), str(mode))

    def daw_mode(self, mode):
        return self._daw.set_mode(str(mode))

    def daw_reconcile(self, choice):
        return self._daw.reconcile_choice(str(choice))

    def daw_project(self):
        return self._daw.view()

    def daw_edit(self, changes):
        return self._daw.edit(list(changes))

    def daw_comp(self, group, take, a, b):
        return self._daw.comp(str(group), int(take), float(a), float(b))

    def daw_take_op(self, group, op, take=None):
        return self._daw.take_op(str(group), str(op), None if take is None else int(take))

    def daw_time_op(self, op, a, b):
        return self._daw.time_op(str(op), float(a), float(b))

    def daw_local(self, key, value, track=None):
        self._daw.set_local(key, value, track)
        return self._daw.view()

    def daw_play(self, pos=None):
        self._daw.play(pos)

    def daw_stop(self):
        self._daw.stop()

    def daw_record(self, pos=None, jam=False):
        return self._daw.record(pos, bool(jam))

    def daw_locate(self, pos):
        self._daw.locate(float(pos))

    def daw_scrub(self, pos, what="move"):
        self._daw.scrub(None if pos is None else float(pos), str(what))

    def daw_peaks(self, name):
        return self._daw.peaks(name)

    def daw_wave(self, name, start, per, n):
        # a stretch of a file's waveform at the zoom it's drawn at: (min, max) i16 pairs, base64
        try:
            b = self._engine.daw_wave(str(name), float(start), float(per), int(n))
        except Exception:
            return None
        return base64.b64encode(b).decode() if b is not None else None

    def daw_analyze(self, clip, what, sensitivity=0.5):
        """For the clip editor: "peak" (its loudest sample), "hits" (where the
        hits are, in seconds of the file) or "tempo" (BPM or None)."""
        return self._daw.analyze(str(clip), str(what), float(sensitivity))

    def tune_audition(self, clip, start, end, center, shift, drift=0.0, mod=1.0, gain=0.0):
        return self._daw.tune_audition(str(clip), start, end, center, shift, drift, mod, gain)

    def tune_audition_stop(self):
        try:
            self._engine.preview(None)
        except Exception:
            pass

    def stems_state(self):
        return self._daw.stems_state()

    def stems_get_model(self):
        return self._daw.stems_get_model()

    def stems_split(self, clip, which="4"):
        return self._daw.split_stems(str(clip), str(which))

    def stems_cancel(self):
        self._daw.stems_cancel()

    def daw_projects(self):
        return self._daw.projects()

    def daw_new(self, opts=None):
        """A new song. opts: its name, or {name, location (None: Duet's Projects folder), bpm, beats,
        key, track (start with an audio track)}."""
        o = opts if isinstance(opts, dict) else {"name": opts}
        r = self._daw.new_project(o.get("name") or "", o.get("location") or None, o.get("bpm"), o.get("beats"),
                                  o.get("key"), o.get("track", True))
        return {**r, "view": self._daw.view()}

    def daw_open(self, pid):
        self._daw.open_project(pid)
        return self._daw.view()

    def daw_open_folder(self):
        """Open a song kept in a folder of its own somewhere (asks which folder)."""
        picked = self._window.create_file_dialog(webview.FileDialog.FOLDER)
        if not picked:
            return None
        r = self._daw.open_folder(picked if isinstance(picked, str) else picked[0])
        return {**r, "view": self._daw.view()}

    def daw_forget(self, pid):
        return self._daw.forget_project(str(pid))

    def daw_save(self):
        return self._daw.save_now()

    def daw_name_free(self, name, but=None):
        """{taken: a song here has that name already, suggest: a free one like it}"""
        return {"taken": self._daw.name_taken(str(name), but), "suggest": self._daw.free_name(str(name))}

    def pick_folder(self, start=None):
        kw = {"directory": str(start)} if start and Path(str(start)).is_dir() else {}
        picked = self._window.create_file_dialog(webview.FileDialog.FOLDER, **kw)
        if not picked:
            return None
        return picked if isinstance(picked, str) else picked[0]

    # --- preferences (settings.json, next to the songs) ------------------------------

    PREFS = {"confirm_close": True, "song_location": "", "new_bpm": 120.0, "new_beats": 4, "auto_update": True,
             "mix_threads": True, "plugin_sandbox": False}

    def prefs_get(self):
        st = load_settings()
        out = {k: st.get(k, v) for k, v in self.PREFS.items()}
        out["projects_dir"] = str(data_dir() / "Projects")
        return out

    def prefs_set(self, key, value):
        if key not in self.PREFS:
            return {"error": f"unknown preference {key}"}
        d = self.PREFS[key]
        value = bool(value) if isinstance(d, bool) else str(value or "") if isinstance(d, str) else float(value)
        if key == "new_beats":
            value = int(min(16, max(1, value)))
        if key == "new_bpm":
            value = min(999.0, max(20.0, value))
        if key == "mix_threads":
            return {"ok": True, "value": self.mix_threads(value)}
        st = load_settings()
        st[key] = value
        save_settings(st)
        return {"ok": True, "value": value}

    # --- closing the window: asked first (the page shows the question) ----------------

    def _closing(self):
        if self._may_close or not load_settings().get("confirm_close", True):
            return True
        # (from the window's own thread: the page is asked on another, or the two wait for each other)
        def ask():
            try:
                shown = self._window.evaluate_js("window.askClose ? (askClose(), true) : false")
            except Exception:
                shown = False
            if not shown:  # the page can't ask (it's broken or still loading): close, saved, as before
                self.app_close(False)
        threading.Thread(target=ask, daemon=True).start()
        return False

    def app_close(self, save=False):
        """The answer to askClose: close (after saving, with save)."""
        r = self._daw.save_now() if save else {"ok": True}
        if r.get("error"):
            return r
        self._may_close = True
        threading.Timer(0.05, self._window.destroy).start()
        return {"ok": True}

    def daw_versions(self):
        return self._daw.versions()

    def daw_restore(self, vid):
        r = self._daw.restore_version(str(vid))
        return {**r, "view": self._daw.view()}

    def daw_save_bundle(self):
        """Save the song and its audio as one .duet file (asks where)."""
        name = re.sub(r'[\\/:*?"<>|]+', "_", self._daw.p.meta()["name"]).strip() or "Song"
        picked = self._window.create_file_dialog(webview.FileDialog.SAVE, save_filename=f"{name}.duet",
                                                 file_types=("Duet song (*.duet)",))
        if not picked:
            return None
        path = picked if isinstance(picked, str) else picked[0]
        if not path.lower().endswith(".duet"):
            path += ".duet"
        return self._daw.save_bundle(path)

    def daw_open_bundle(self):
        """Open a .duet song file (asks which)."""
        picked = self._window.create_file_dialog(webview.FileDialog.OPEN, allow_multiple=False,
                                                 file_types=("Duet song (*.duet)", "All files (*.*)"))
        if not picked:
            return None
        r = self._daw.open_bundle(picked[0] if not isinstance(picked, str) else picked)
        return {**r, "view": self._daw.view()}

    def daw_resolve(self, choice):
        self._daw.resolve(choice)
        return self._daw.view()

    def daw_presence(self, here):
        self._daw.set_presence(here)

    def midi_import(self, pattern=None, track=None, at=0.0):
        """Pick a .mid file and bring it in: into a pattern's channel (the piano roll), or as new
        channels and a pattern in the playlist."""
        picked = self._window.create_file_dialog(webview.FileDialog.OPEN, allow_multiple=False,
                                                 file_types=("MIDI files (*.mid;*.midi)", "All files (*.*)"))
        if not picked:
            return None
        path = picked[0] if isinstance(picked, (list, tuple)) else picked
        return self._daw.import_midi(path, pattern or None, track or None, float(at or 0.0))

    def midi_export(self, pattern=None):
        """The song's notes (or a pattern's) as a .mid file, wherever you choose."""
        name = (self._daw.p.meta().get("name") or "Song").strip() or "Song"
        picked = self._window.create_file_dialog(webview.FileDialog.SAVE, save_filename=f"{name}.mid")
        if not picked:
            return None
        path = picked[0] if isinstance(picked, (list, tuple)) else picked
        if not str(path).lower().endswith((".mid", ".midi")):
            path = str(path) + ".mid"
        return self._daw.export_midi(path, pattern or None)

    def daw_play_pattern(self, pid, at=0.0):
        self._daw.play_pattern(pid, float(at or 0.0))

    def daw_undo(self):
        return self._daw.undo()

    def daw_redo(self):
        return self._daw.redo()

    def daw_export(self, opts):
        """Ask where, then render: {"stems": bool, "loop": bool}."""
        if not self._window:
            return {"error": "No window to ask where to save."}
        name = file_name(self._daw.view()["meta"]["name"])
        if opts.get("stems"):
            picked = self._window.create_file_dialog(webview.FileDialog.FOLDER)
            if not picked:
                return {"cancelled": True}
            folder = Path(picked[0] if isinstance(picked, (list, tuple)) else picked) / f"{name} (stems)"
            try:
                folder.mkdir(parents=True, exist_ok=True)
            except OSError as exc:
                return {"error": str(exc)}
            mix, stems = str(folder / f"{name}.wav"), str(folder)
        else:
            picked = self._window.create_file_dialog(webview.FileDialog.SAVE, save_filename=f"{name}.wav",
                                                     file_types=("WAV audio (*.wav)",))
            if not picked:
                return {"cancelled": True}
            mix = picked[0] if isinstance(picked, (list, tuple)) else picked
            if not mix.lower().endswith(".wav"):
                mix += ".wav"
            stems = None
        norm = opts.get("normalize")
        norm = float(norm) if isinstance(norm, (int, float)) and -40 <= norm <= 0 else None
        return self._daw.export(mix, stems, bool(opts.get("loop")), norm)

    def daw_export_status(self):
        return self._engine.daw_export_status()

    def show_file(self, path):
        """Show a file in Finder / Explorer."""
        try:
            if sys.platform == "darwin":
                subprocess.run(["open", "-R", path], check=False)
            elif sys.platform == "win32":
                subprocess.run(["explorer", "/select,", os.path.normpath(path)], check=False)
            else:
                subprocess.run(["xdg-open", os.path.dirname(path)], check=False)
        except OSError:
            pass

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
        self._rack_changed(target)
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

    def vst_plugins(self, instruments=False):
        return {"plugins": self._vst.plugins(bool(instruments)), "failed": self._vst.failed(), "status": self._vst.status}

    def vst_add(self, target, path, cid, name):
        """Load a plugin into a rack. Returns the mixer, or {"error": ...}."""
        try:
            self._engine.rack_add_vst(target, path, cid, name)
        except Exception as exc:
            return {"error": str(exc)}
        self._rack_changed(target)
        return self._engine.mixer()

    # --- instruments: a strip plays one instead of its input -------------------

    def _inst_changed(self, target):
        # a song track's instrument is part of the song (channels in the sequencer)
        if str(target).startswith("T"):
            self._daw.track_inst_changed(int(target[1:]))

    def inst_set(self, target, kind=None, vst=None):
        """kind "drums" / "synth" / "vst" (vst = {"path", "cid", "name"}), or None
        for the input again. Returns the mixer, or {"error": ...}."""
        try:
            ref = (vst["path"], vst["cid"], vst["name"]) if vst else None
            self._engine.inst_set(target, kind or None, ref)
        except Exception as exc:
            return {"error": str(exc)}
        self._inst_changed(target)
        return self._engine.mixer()

    def inst_param(self, target, key, value):
        self._engine.inst_param(target, key, float(value))
        self._inst_changed(target)

    def inst_pad(self, target, pad, path=None):
        """Your own sample on a drum pad (None: the built-in sound again). On a
        song channel it goes into the project, so your partner gets it too."""
        if str(target).startswith("T"):
            tid = next((t for t, s in self._daw.slots.items() if s == int(target[1:])), None)
            res = self._daw.sample_pad(tid, int(pad), path) if tid else {"error": "No such track."}
            if path and res.get("ok"):
                self._lib.used(path)
            return res if res.get("error") else self._engine.mixer()
        try:
            self._engine.inst_pad(target, int(pad), path)
        except Exception as exc:
            return {"error": str(exc)}
        self._inst_changed(target)
        return self._engine.mixer()

    # --- the sample library (the browser) -------------------------------------------

    def lib_roots(self):
        return {"roots": self._lib.roots(), "bins": self._lib.bins(), "recent": self._lib.recent()}

    def lib_list(self, path):
        return self._lib.listing(path)

    def lib_bin(self, bid):
        return self._lib.bin_listing(bid)

    def lib_search(self, query):
        return self._lib.search(query)

    def lib_add_folder(self):
        """Pick a folder on disk (your sample packs) to show in the browser."""
        if not self._window:
            return None
        picked = self._window.create_file_dialog(webview.FileDialog.FOLDER)
        return self._lib.add_folder(picked[0]) if picked else None

    def lib_remove_folder(self, path):
        self._lib.remove_folder(path)
        return self.lib_roots()

    def lib_new_folder(self, parent, name):
        return self._lib.new_folder(parent, name)

    def lib_rename(self, path, name):
        return self._lib.rename(path, name)

    def lib_import(self, dest=None):
        """Pick sounds on disk and copy them into My samples (or `dest`)."""
        if not self._window:
            return None
        picked = self._window.create_file_dialog(webview.FileDialog.OPEN, allow_multiple=True,
                                                 file_types=("Audio (*.wav;*.mp3;*.m4a;*.aac;*.flac;*.aif;*.aiff;*.ogg)", "All files (*.*)"))
        return self._lib.import_files(list(picked or []), dest)

    def lib_copy(self, path, dest):
        """A sound into one of your folders (copied)."""
        res = self._lib.import_files([str(path)], dest)
        return res if res.get("files") else {"error": "Couldn't copy that there."}

    def lib_new_bin(self, name, parent=None):
        return self._lib.new_bin(name, parent)

    def lib_rename_bin(self, bid, name):
        self._lib.rename_bin(bid, name)

    def lib_delete_bin(self, bid):
        self._lib.delete_bin(bid)

    def lib_move_bin(self, bid, parent=None):
        self._lib.move_bin(bid, parent)

    def lib_bin_add(self, bid, paths):
        return self._lib.bin_add(bid, list(paths))

    def lib_bin_remove(self, bid, path):
        self._lib.bin_remove(bid, path)

    def lib_info(self, path):
        """Length, rate, channels and a waveform (base64 peaks) of a sound."""
        try:
            frames, sr, ch, peaks = Engine.wav_info(str(path))
        except Exception as exc:
            return {"error": str(exc)}
        import base64
        return {"frames": frames, "sr": sr, "channels": ch, "peaks": base64.b64encode(peaks).decode()}

    def lib_preview(self, path=None):
        try:
            self._engine.preview(str(path) if path else None)
            return {"ok": True}
        except Exception as exc:
            return {"error": str(exc)}

    def lib_preview_state(self):
        return self._engine.preview_state

    def lib_to_song(self, path, track=None, start=0.0):
        """A sound into the playlist: on `track` at `start` (beats), or a new track."""
        res = self._daw.sample_clip(str(path), track, float(start))
        if res.get("ok"):
            self._lib.used(str(path))
        return res

    def lib_to_channel(self, path):
        """A sound as a new sampler channel (plays it at each note's pitch)."""
        res = self._daw.sampler_channel(str(path))
        if res.get("ok"):
            self._lib.used(str(path))
        return res

    def inst_pad_open(self):
        """Native file picker for a sample; returns the path or None."""
        if not self._window:
            return None
        picked = self._window.create_file_dialog(webview.FileDialog.OPEN, allow_multiple=False,
                                                 file_types=("Audio (*.wav;*.mp3;*.m4a;*.aac;*.flac;*.aif;*.aiff;*.ogg)", "All files (*.*)"))
        return picked[0] if picked else None

    def note(self, target, on, key, vel=100):
        self._engine.note(target, bool(on), int(key), int(vel))

    def midi_inputs(self):
        return self._engine.midi_inputs()

    def midi_open(self, name=None):
        try:
            self._engine.midi_open(name or None)
            return {"ok": True}
        except Exception as exc:
            return {"error": str(exc)}

    def midi_target(self, target=None):
        self._engine.midi_target(target or None)

    def plugin_params(self, target, pos):
        # a VST3/AU's automatable parameters (pos -1: the strip's instrument)
        try:
            return self._engine.plugin_params(target, int(pos))
        except Exception:
            return []

    def plugin_touched(self, target, pos):
        # the parameter last moved in the plugin's own window (its id), or None
        try:
            return self._engine.plugin_touched(target, int(pos))
        except Exception:
            return None

    def vst_open(self, target, pos):
        try:
            self._engine.vst_open_editor(target, int(pos))  # pos -1: the strip's instrument
            return {"ok": True}
        except Exception as exc:
            return {"error": str(exc)}

    # --- mixer persistence: a file, so plugin settings changed in their own
    #     windows are saved too (on every change and when Duet closes)

    def save_mixer(self, names=None):
        if names is not None:
            self._names = names
        snap = {"mixer": self._engine.mixer(True), "names": self._names}
        self._daw.sync_rack_states(snap["mixer"])
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

    # --- updating itself (updater.py) ---------------------------------------------

    def update_check(self):
        return self._upd.check()

    def update_download(self):
        return self._upd.download()

    def update_status(self):
        return self._upd.status()

    def update_install(self):
        # the song is saved as it closes; the new version starts once this one is gone
        r = self._upd.install()
        if r.get("ok") and self._window:
            self._may_close = True  # (the song is saved as it closes: no question)
            threading.Timer(0.3, self._window.destroy).start()
        return r

    # --- talkback: a talk mic that only ever goes to the partner's ears --------------

    def talk_config(self, input_=None):
        """Which interface input is the talk mic (None: none); kept for next time."""
        sel = None if input_ is None or int(input_) < 0 else int(input_)
        self._engine.talk_input(sel)
        self._talk_input = sel
        settings = load_settings()
        settings["talk_input"] = sel
        save_settings(settings)
        return {"input": sel}

    def mix_threads(self, on):
        """The song's tracks on all cores (True) or one; kept for next time."""
        settings = load_settings()
        settings["mix_threads"] = bool(on)
        save_settings(settings)
        return self._engine.mix_threads(bool(on))

    def mix_threads_get(self):
        return load_settings().get("mix_threads", True)

    def talk(self, on):
        self._engine.talk(bool(on))

    def shutdown(self):
        self.save_mixer()  # plugin windows may have changed settings since the last save
        self._daw.close()
        self._engine.close()


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "--vst-scan":
        scan_main(sys.argv[2], sys.argv[3])  # child process of the plugin scanner; exits
    if sys.platform == "darwin":
        # plugin windows must live on the main thread, which pywebview runs
        os.environ["DUET_VST_MAIN_THREAD"] = "1"
    ap = argparse.ArgumentParser(description="Duet: peer-to-peer jamming")
    ap.add_argument("--port", type=int, default=0, help="fixed UDP port (default: the one from last time)")
    ap.add_argument("--debug", action="store_true", help="open the webview dev tools")
    args = ap.parse_args()
    # plugins in a process of their own (a crash there doesn't take Duet down)
    # (off by default until it's been tried on Mac and Windows: settings.json "plugin_sandbox": true)
    Engine.plugin_sandbox(plugin_host_command(), bool(load_settings().get("plugin_sandbox", False)))
    engine = open_engine(args.port)
    engine.start()
    api = Api(engine)
    window = webview.create_window("Duet", url=str(resource("ui/index.html")), js_api=api,
                                   width=1280, height=900, min_size=(980, 780),
                                   background_color="#0e0d12")
    api._window = window
    window.events.closing += api._closing
    window.events.closed += api.shutdown
    clear_stale_web_cache()
    # Served from http://127.0.0.1:<fixed port> rather than file://: the camera needs a
    # secure origin, and a stable origin + persistent storage keep saved settings and
    # the camera permission across launches.
    webview.start(debug=args.debug, http_server=True, http_port=pick_port(),
                  private_mode=False, storage_path=str(data_dir() / "webview"))


def plugin_host_command() -> list:
    """How to start Duet as its plugin process (it gets --plugin-host <file> <tag> after this)."""
    if getattr(sys, "frozen", False):
        return [sys.executable]
    return [sys.executable, str(Path(__file__).resolve())]


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


def clear_stale_web_cache() -> None:
    """After an update, throw away the webview's HTTP cache before the page loads.

    The page always comes from the same origin (see pick_port), and WebKit on macOS kept serving
    the previous version's scripts from its disk cache despite the server's no-cache headers: a
    new Duet.app showed the old UI. So when the UI files differ from last launch's, the cache goes
    (only the cache: saved settings and the camera permission live elsewhere and stay).
    """
    h = hashlib.sha256()
    ui = resource("ui")
    for f in sorted(p for p in ui.rglob("*") if p.is_file()):
        h.update(f.relative_to(ui).as_posix().encode())
        h.update(f.read_bytes())
    stamp, stamp_file = h.hexdigest(), data_dir() / "ui.stamp"
    try:
        if stamp_file.read_text(encoding="utf-8") == stamp:
            return
    except OSError:
        pass
    if sys.platform == "darwin":
        caches = [Path.home() / "Library" / "Caches" / "app.duet.jam" / "WebKit"]
    else:  # WebView2 keeps its profile in storage_path
        profile = data_dir() / "webview" / "EBWebView" / "Default"
        caches = [profile / "Cache", profile / "Code Cache", profile / "Service Worker" / "CacheStorage"]
    for c in caches:
        shutil.rmtree(c, ignore_errors=True)
    try:
        stamp_file.write_text(stamp, encoding="utf-8")
    except OSError:
        pass


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
