"""Sound card side: device listing and the duplex stream that feeds the engine."""
from __future__ import annotations

import os
import queue
import sys
import threading

os.environ.setdefault("SD_ENABLE_ASIO", "1")  # must be set before importing sounddevice

import numpy as np  # noqa: E402
import sounddevice as sd  # noqa: E402

PREFERRED_APIS = ["ASIO", "Windows WASAPI", "Core Audio", "ALSA", "JACK Audio Connection Kit"]
# Software ASIO wrappers sit on top of the Windows mixer with big buffers; a real
# interface driver (Focusrite, Behringer, ...) should win the default pick.
WRAPPER_DRIVERS = ("fl studio", "asio4all", "generic low latency", "voicemeeter", "realtek asio")


def list_devices():
    """Inputs and outputs as dicts, lowest-latency APIs and real interfaces first."""
    apis = sd.query_hostapis()
    rows = []
    for idx, d in enumerate(sd.query_devices()):
        api = apis[d["hostapi"]]["name"]
        rank = PREFERRED_APIS.index(api) if api in PREFERRED_APIS else len(PREFERRED_APIS)
        wrapper = any(w in d["name"].lower() for w in WRAPPER_DRIVERS)
        rows.append((rank, wrapper, idx, {
            "id": idx, "name": d["name"], "api": api.replace("Windows ", ""),
            "inputs": d["max_input_channels"], "outputs": d["max_output_channels"],
        }))
    rows.sort(key=lambda r: r[:3])
    devs = [r[3] for r in rows]
    return [d for d in devs if d["inputs"] > 0], [d for d in devs if d["outputs"] > 0]


def default_devices(inputs, outputs):
    """(input id, output id): an ASIO interface with both directions, else the OS default."""
    both = {d["id"] for d in inputs} & {d["id"] for d in outputs}
    for d in inputs:
        if d["api"] == "ASIO" and d["id"] in both and not any(w in d["name"].lower() for w in WRAPPER_DRIVERS):
            return d["id"], d["id"]
    din, dout = sd.default.device
    ids_in, ids_out = [d["id"] for d in inputs], [d["id"] for d in outputs]
    return (din if din in ids_in else (ids_in[0] if ids_in else None),
            dout if dout in ids_out else (ids_out[0] if ids_out else None))


class _StaThread:
    """Runs calls on one long-lived thread that is a COM single-threaded apartment.

    ASIO drivers are COM objects that only load on an STA thread; the webview calls
    our API from its own (MTA / uninitialized) threads, where opening fails with
    "Failed to load ASIO driver". On macOS/Linux this is just a worker thread.
    """

    def __init__(self):
        self._jobs: queue.Queue = queue.Queue()
        threading.Thread(target=self._loop, daemon=True, name="audio-control").start()

    def _loop(self):
        if sys.platform == "win32":
            import ctypes
            ctypes.windll.ole32.CoInitializeEx(None, 2)  # COINIT_APARTMENTTHREADED
        while True:
            fn, box, done = self._jobs.get()
            try:
                box["result"] = fn()
            except BaseException as exc:  # handed back to the caller
                box["error"] = exc
            done.set()

    def call(self, fn):
        box, done = {}, threading.Event()
        self._jobs.put((fn, box, done))
        done.wait()
        if "error" in box:
            raise box["error"]
        return box.get("result")


class AudioIO:
    """Owns the sounddevice stream; calls engine.process once per block."""

    def __init__(self, engine):
        self.engine = engine
        self.stream: sd.Stream | None = None
        self.in_channel: int | None = 0   # None = mix all inputs
        self.xruns = 0
        self._sta = _StaThread()

    @property
    def running(self) -> bool:
        return self.stream is not None

    @property
    def latency_ms(self) -> tuple[float, float]:
        if not self.stream:
            return 0.0, 0.0
        lin, lout = self.stream.latency
        return lin * 1000, lout * 1000

    def start(self, in_id: int, out_id: int, channel, samplerate: int, block: int):
        self._sta.call(lambda: self._start(in_id, out_id, channel, samplerate, block))

    def stop(self):
        self._sta.call(self._stop)

    def _start(self, in_id, out_id, channel, samplerate, block):
        self._stop()
        in_dev, out_dev = sd.query_devices(in_id), sd.query_devices(out_id)
        self.in_channel = None if channel == "mix" else int(channel)
        in_channels = in_dev["max_input_channels"] if self.in_channel is None else self.in_channel + 1
        out_channels = min(2, out_dev["max_output_channels"])
        is_asio = sd.query_hostapis(in_dev["hostapi"])["name"] == "ASIO"
        # ASIO: ask for the driver's minimum buffer. PortAudio's "low" preset asks a
        # Focusrite for ~38 ms each way vs ~7 ms at 0. Other APIs glitch at 0.
        latency = 0.0 if is_asio else "low"

        self.engine.set_format(samplerate, block)
        self.xruns = 0
        stream = sd.Stream(device=(in_id, out_id), samplerate=samplerate, blocksize=block,
                           channels=(in_channels, out_channels), dtype="float32",
                           latency=latency, callback=self._callback)
        stream.start()
        self.stream = stream

    def _stop(self):
        if self.stream:
            self.stream.stop()
            self.stream.close()
            self.stream = None

    def _callback(self, indata, outdata, _frames, _time, status):
        if status:
            self.xruns += 1
        mono = indata[:, self.in_channel] if self.in_channel is not None else indata.mean(axis=1)
        outdata[:] = self.engine.process(np.ascontiguousarray(mono))[:, None]
