"""Duet engine: UDP peer link + jitter buffer + per-block audio processing.

Kept free of any audio-device or GUI code so selftest.py can drive two engines
against each other with synthetic audio. The audio callback calls
`Engine.process(block)` once per block; everything network-side runs on two
background threads.
"""
from __future__ import annotations

import random
import socket
import threading
import time

import numpy as np

import net
from effects import Chain, Reverb

CONNECT_TIMEOUT = 4.0      # seconds of silence before we call the partner gone
HELLO_INTERVAL = 0.1       # while punching
PING_INTERVAL = 1.0
TRIM_WINDOW = 200          # blocks (~0.5 s at 128/48k) to watch before trimming drift
HARD_TRIM = 32             # frames over target that get trimmed right away


class JitterBuffer:
    """Reorders incoming frames and plays them out at a fixed depth.

    `target` is how many frames we hold back before playing. More = fewer
    dropouts on a jittery connection, but each frame adds frame/samplerate of
    delay (128 @ 48k = 2.7 ms).
    """

    def __init__(self, target: int = 3):
        self.target = target
        self._lock = threading.Lock()
        self.reset()
        self.underruns = 0    # buffer ran dry (network gap or clock drift)
        self.lost = 0         # frame never arrived
        self.late = 0         # frame arrived after its playout slot
        self.dropped = 0      # frames skipped to bring the buffer back down

    def reset(self):
        with self._lock:
            self._frames: dict[int, np.ndarray] = {}
            self._next: int | None = None
            self._highest = -1
            self._started = False
            self._starved = 0
            self._win_min, self._win_count = 1 << 30, 0

    @property
    def depth(self) -> int:
        if self._next is None:
            return len(self._frames)
        return max(0, self._highest - self._next + 1)

    def push(self, seq: int, frame: np.ndarray):
        with self._lock:
            if self._next is not None and abs(seq - self._next) > 5000:
                # partner restarted their audio (seq counter jumped): start over
                self._frames.clear()
                self._next, self._highest, self._started = None, -1, False
            if self._next is not None and seq < self._next:
                self.late += 1
                return
            self._frames[seq] = frame
            if seq > self._highest:
                self._highest = seq

    def pop(self) -> np.ndarray | None:
        """Next frame for playout, or None when there's nothing to play."""
        with self._lock:
            if not self._started:
                if not self._frames:
                    return None
                first = min(self._frames)
                if self._highest - first + 1 < self.target:
                    return None
                self._next = max(first, self._highest - self.target + 1)
                self._started = True
                self._starved = 0

            # Too much queued up. Sound card drivers often hand out audio in bursts
            # (an ASIO buffer of 512 = 4 of our blocks back to back), so the depth
            # swings by a few frames all the time; only trim when even the lowest
            # depth over ~half a second sits above target (the partner's clock is
            # a bit fast), or immediately after a huge pile-up.
            depth = self._highest - self._next + 1
            self._win_min = min(self._win_min, depth)
            self._win_count += 1
            excess = 0
            if depth > self.target + HARD_TRIM:
                excess = depth - self.target
            elif self._win_count >= TRIM_WINDOW:
                if self._win_min > self.target + 1:
                    excess = self._win_min - self.target
                self._win_min, self._win_count = 1 << 30, 0
            if excess:
                new_next = self._next + excess
                self.dropped += excess
                for s in range(self._next, new_next):
                    self._frames.pop(s, None)
                self._next = new_next
                self._win_min, self._win_count = 1 << 30, 0

            frame = self._frames.pop(self._next, None)
            if frame is not None:
                self._next += 1
                self._starved = 0
                return frame
            if self._highest > self._next:
                # later frames are here, so this one is lost: move on
                self.lost += 1
                self._next += 1
                return None
            # Nothing newer either: the buffer ran dry. Don't advance, so a frame
            # that is merely late still gets played (this re-adds a frame of delay,
            # which is also what corrects for a slow partner clock).
            self.underruns += 1
            self._starved += 1
            if self._starved > 400:   # ~1 s of nothing: rebuffer from scratch
                self._frames.clear()
                self._next, self._highest, self._started = None, -1, False
            return None


class Engine:
    def __init__(self, samplerate: int = 48000, frame: int = 128, port: int = 0):
        self.samplerate = samplerate
        self.frame = frame
        self.remote_gain = 1.0
        self.monitor_gain = 0.0
        self.jitter = JitterBuffer(target=3)

        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1 << 20)
        if hasattr(socket, "SIO_UDP_CONNRESET"):
            # Windows: otherwise an ICMP "port unreachable" (partner not up yet)
            # makes the next recvfrom raise ConnectionResetError.
            self.sock.ioctl(socket.SIO_UDP_CONNRESET, False)
        self.sock.bind(("0.0.0.0", port))
        self.local_addr = (net.local_ip(), self.sock.getsockname()[1])
        self.public_addr = None

        self.candidates: list[tuple[str, int]] = []
        self.peer_addr: tuple[str, int] | None = None
        self.last_heard = 0.0
        self.rtt_ms: float | None = None
        self.warning = ""
        self.packets_in = 0

        self._seq = 0
        self._ping_seq = 0
        self._plc_gain = 0.0
        self._last_frame = np.zeros(frame, dtype=np.float32)
        self.in_level = 0.0
        self.out_level = 0.0
        self.fx = Chain(samplerate)                 # my pedalboard, applied before sending
        self.room = Chain(samplerate, [Reverb])     # optional reverb on the partner
        self.fx_error = ""
        self._running = False
        self._threads: list[threading.Thread] = []

        self._stun_txid = b""
        self._stun_reply: bytes | None = None
        self._stun_event = threading.Event()

        # reliable signaling side channel (video call setup)
        self._sig_lock = threading.Lock()
        self._sig_next_id = random.getrandbits(31)
        self._sig_outbox: dict[int, list] = {}          # id -> [packets, next_send, tries]
        self._sig_partial: dict[int, dict[int, bytes]] = {}
        self._sig_done: dict[int, float] = {}            # ids already delivered -> when
        self._sig_inbox: list[str] = []

    # --- lifecycle -----------------------------------------------------------

    def discover(self) -> str:
        """Look up our public address and return the code to give the partner.

        Must go out of the same socket we send audio from: the router mapping is
        per socket, so the port STUN sees is the port the partner must send to.
        The reply is picked up by the receive loop (only one thread may read the
        socket), so call this after start().
        """
        self.public_addr = None
        for addr in net.resolve_stun_servers():
            for _attempt in range(2):
                txid, req = net.stun_request()
                self._stun_txid, self._stun_reply = txid, None
                self._stun_event.clear()
                self._send(req, addr)
                if self._stun_event.wait(1.5) and self._stun_reply:
                    self.public_addr = net.parse_stun_response(self._stun_reply)
                    if self.public_addr:
                        return net.make_code(self.public_addr, self.local_addr)
        return net.make_code(None, self.local_addr)

    def start(self):
        self._running = True
        for target in (self._recv_loop, self._control_loop):
            t = threading.Thread(target=target, daemon=True)
            t.start()
            self._threads.append(t)

    def set_format(self, samplerate: int, frame: int):
        """Called before (re)starting audio. Rebuilds effects if the rate changed."""
        if samplerate != self.samplerate:
            self.fx = self._rebuilt(self.fx, Chain(samplerate))
            self.room = self._rebuilt(self.room, Chain(samplerate, [Reverb]))
        self.samplerate, self.frame = samplerate, frame
        self.jitter.reset()

    @staticmethod
    def _rebuilt(old: Chain, new: Chain) -> Chain:
        for name, e in old.effects.items():
            new.effects[name].p = dict(e.p)
            new.effects[name].on = e.on
        return new

    def connect(self, code: str):
        self.candidates = net.parse_code(code)
        self.peer_addr = None
        self.jitter.reset()

    # --- signaling side channel ----------------------------------------------
    # Small messages that must arrive (WebRTC offers/answers/candidates). Split
    # into chunks, resent every 300 ms until the partner acks the whole message.

    def send_signal(self, text: str):
        data = text.encode()
        n = net.SIGNAL_CHUNK
        chunks = [data[i:i + n] for i in range(0, len(data), n)] or [b""]
        with self._sig_lock:
            mid = self._sig_next_id
            self._sig_next_id = (self._sig_next_id + 1) & 0x7FFFFFFF
            packets = [net.pack(net.T_SIGNAL, mid, net.CHUNK_HEAD.pack(i, len(chunks)) + c)
                       for i, c in enumerate(chunks)]
            self._sig_outbox[mid] = [packets, 0.0, 0]
        self._flush_signals()

    def drain_signals(self) -> list[str]:
        with self._sig_lock:
            out, self._sig_inbox = self._sig_inbox, []
        return out

    def _flush_signals(self):
        now = time.monotonic()
        with self._sig_lock:
            due = []
            for mid, entry in list(self._sig_outbox.items()):
                packets, next_send, tries = entry
                if tries >= 30:          # ~9 s without an ack: give up
                    del self._sig_outbox[mid]
                elif now >= next_send:
                    entry[1], entry[2] = now + 0.3, tries + 1
                    due.extend(packets)
        for p in due:
            self._send(p)

    def _on_signal_chunk(self, mid: int, body: bytes):
        if len(body) < net.CHUNK_HEAD.size:
            return
        idx, count = net.CHUNK_HEAD.unpack_from(body)
        with self._sig_lock:
            if mid not in self._sig_done:
                parts = self._sig_partial.setdefault(mid, {})
                parts[idx] = body[net.CHUNK_HEAD.size:]
                if len(parts) == count:
                    del self._sig_partial[mid]
                    self._sig_done[mid] = time.monotonic()
                    self._sig_inbox.append(b"".join(parts[i] for i in range(count)).decode("utf-8", "replace"))
            complete = mid in self._sig_done
            # forget old ids so the dict doesn't grow forever
            if len(self._sig_done) > 512:
                cutoff = time.monotonic() - 60
                self._sig_done = {k: t for k, t in self._sig_done.items() if t > cutoff}
        if complete:   # (re-)ack: our previous ack may have been lost
            self._send(net.pack(net.T_SIGACK, mid))

    def _reset_signals(self):
        with self._sig_lock:
            self._sig_outbox.clear()
            self._sig_partial.clear()

    def close(self):
        if self.peer_addr:
            try:
                self.sock.sendto(net.pack(net.T_BYE, 0), self.peer_addr)
            except OSError:
                pass
        self._running = False
        self.sock.close()

    @property
    def connected(self) -> bool:
        return self.peer_addr is not None and time.monotonic() - self.last_heard < CONNECT_TIMEOUT

    # --- audio thread --------------------------------------------------------

    def process(self, mono_in: np.ndarray) -> np.ndarray:
        """Called once per audio block: send our input, return what to play."""
        try:
            mono_in = self.fx.process(mono_in)
        except Exception as exc:  # never let an effect bug kill the audio stream
            self.fx_error = f"effects off after error: {exc!r}"
            for e in self.fx.effects.values():
                e.on = False
        peer = self.peer_addr
        if peer is not None:
            pcm = (np.clip(mono_in, -1.0, 1.0) * 32767).astype("<i2").tobytes()
            try:
                self.sock.sendto(net.pack(net.T_AUDIO, self._seq, pcm), peer)
            except OSError:
                pass
            self._seq += 1

        remote = self.jitter.pop()
        if remote is None or len(remote) != len(mono_in):
            # Concealment: fade the last good frame out instead of a hard click.
            self._plc_gain *= 0.5
            remote = self._last_frame * self._plc_gain
        else:
            self._last_frame = remote
            self._plc_gain = 1.0
        if self.room.effects["reverb"].on:
            remote = self.room.process(remote)

        out = remote * self.remote_gain
        if self.monitor_gain:
            out = out + mono_in * self.monitor_gain
        self.in_level = max(float(np.max(np.abs(mono_in))), self.in_level * 0.9)
        self.out_level = max(float(np.max(np.abs(remote))), self.out_level * 0.9)
        return out.astype(np.float32, copy=False)

    # --- network threads -----------------------------------------------------

    def _recv_loop(self):
        self.sock.settimeout(0.5)
        while self._running:
            try:
                data, src = self.sock.recvfrom(4096)
            except socket.timeout:
                continue
            except OSError:
                if not self._running:
                    return
                continue
            pkt = net.unpack(data)
            if pkt is None:
                if net.is_stun_response(data) and data[8:20] == self._stun_txid:
                    self._stun_reply = data
                    self._stun_event.set()
                continue
            ptype, seq, body = pkt

            if ptype == net.T_HELLO:
                # Whichever address the partner's packets actually arrive from is
                # the one their router opened for us; lock onto it.
                if self.peer_addr != src:
                    self.peer_addr = src
                    self.jitter.reset()
                    self._reset_signals()
                self.last_heard = time.monotonic()
                if len(body) >= net.HELLO_BODY.size:
                    sr, fr = net.HELLO_BODY.unpack_from(body)
                    if (sr, fr) != (self.samplerate, self.frame):
                        self.warning = (f"Partner runs {sr} Hz / {fr} buffer, you run "
                                        f"{self.samplerate} Hz / {self.frame}. Match them.")
                    else:
                        self.warning = ""
                continue

            if src != self.peer_addr:
                continue
            self.last_heard = time.monotonic()

            if ptype == net.T_AUDIO:
                self.packets_in += 1
                self.jitter.push(seq, np.frombuffer(body, dtype="<i2").astype(np.float32) / 32767.0)
            elif ptype == net.T_PING:
                self._send(net.pack(net.T_PONG, seq, body))
            elif ptype == net.T_PONG and len(body) >= net.TIME_BODY.size:
                (t0,) = net.TIME_BODY.unpack_from(body)
                rtt = (time.monotonic() - t0) * 1000
                self.rtt_ms = rtt if self.rtt_ms is None else self.rtt_ms * 0.8 + rtt * 0.2
            elif ptype == net.T_SIGNAL:
                self._on_signal_chunk(seq, body)
            elif ptype == net.T_SIGACK:
                with self._sig_lock:
                    self._sig_outbox.pop(seq, None)
            elif ptype == net.T_BYE:
                self.peer_addr = None
                self.rtt_ms = None

    def _control_loop(self):
        next_ping = 0.0
        while self._running:
            now = time.monotonic()
            # rebuilt each pass: samplerate/frame change when the user restarts audio
            hello = net.pack(net.T_HELLO, 0, net.HELLO_BODY.pack(self.samplerate, self.frame))
            if not self.connected:
                # Punch: keep firing at every candidate until one gets through.
                for addr in self.candidates:
                    self._send(hello, addr)
                if self.peer_addr:
                    self._send(hello)
                time.sleep(HELLO_INTERVAL)
                continue
            if now >= next_ping:
                self._send(hello)   # keeps the partner's view of us fresh
                self._send(net.pack(net.T_PING, self._ping_seq, net.TIME_BODY.pack(now)))
                self._ping_seq += 1
                next_ping = now + PING_INTERVAL
            self._flush_signals()
            time.sleep(0.05)

    def _send(self, data: bytes, addr=None):
        addr = addr or self.peer_addr
        if addr is None:
            return
        try:
            self.sock.sendto(data, addr)
        except OSError:
            pass
