"""Standard MIDI files (.mid): read and write, no other libraries.

read(path) gives the file's tempo, time signature and tracks, each with its notes (start and length
in beats, key, velocity, channel), controller moves and pitch bends. write(path, tracks, bpm, beats)
makes a type 1 file with a track per entry. Times are in beats (quarter notes).
"""

from __future__ import annotations

import struct
from pathlib import Path


class MidiError(ValueError):
    pass


def _vlq(data: bytes, i: int) -> tuple[int, int]:
    v = 0
    for _ in range(4):
        if i >= len(data):
            raise MidiError("the file ends in the middle of a number")
        b = data[i]
        i += 1
        v = (v << 7) | (b & 0x7F)
        if not b & 0x80:
            return v, i
    raise MidiError("a number that's too long")


def _vlq_bytes(v: int) -> bytes:
    out = [v & 0x7F]
    v >>= 7
    while v:
        out.append((v & 0x7F) | 0x80)
        v >>= 7
    return bytes(reversed(out))


def read(path: str | Path) -> dict:
    """{"tpq", "bpm", "beats", "tracks": [{"name", "channel", "notes": [(start, len, key, vel)],
    "cc": [(beat, number, value 0..127)], "bend": [(beat, -1..1)]}]} (tracks without notes, controllers
    or bends are left out; a type 0 file's channels become tracks of their own)."""
    data = Path(path).read_bytes()
    if data[:4] != b"MThd":
        raise MidiError("not a MIDI file")
    hlen = struct.unpack(">I", data[4:8])[0]
    fmt, ntrk, div = struct.unpack(">HHH", data[8:14])
    if div & 0x8000:
        raise MidiError("SMPTE timing isn't supported (only beats)")
    tpq = div or 480
    i = 8 + hlen
    bpm, beats = None, None
    raw_tracks = []
    for _ in range(ntrk):
        if data[i:i + 4] != b"MTrk":
            # skip unknown chunks
            if i + 8 > len(data):
                break
            i += 8 + struct.unpack(">I", data[i + 4:i + 8])[0]
            continue
        n = struct.unpack(">I", data[i + 4:i + 8])[0]
        chunk = data[i + 8:i + 8 + n]
        i += 8 + n
        events, name, t, j, status = [], "", 0, 0, None
        while j < len(chunk):
            dt, j = _vlq(chunk, j)
            t += dt
            if j >= len(chunk):
                break
            b = chunk[j]
            if b == 0xFF:  # meta
                typ = chunk[j + 1]
                ln, j = _vlq(chunk, j + 2)
                body = chunk[j:j + ln]
                j += ln
                if typ == 0x51 and len(body) == 3 and bpm is None:
                    bpm = 60_000_000 / int.from_bytes(body, "big")
                elif typ == 0x58 and len(body) >= 2 and beats is None:
                    beats = body[0]
                elif typ == 0x03 and not name:
                    name = body.decode("latin-1", "replace").strip()
                elif typ == 0x2F:
                    break
                continue
            if b in (0xF0, 0xF7):  # sysex
                ln, j = _vlq(chunk, j + 1)
                j += ln
                continue
            if b & 0x80:
                status = b
                j += 1
            elif status is None:
                raise MidiError("a message without a status")
            kind, ch = status & 0xF0, status & 0x0F
            size = 1 if kind in (0xC0, 0xD0) else 2
            d = chunk[j:j + size]
            j += size
            events.append((t, kind, ch, d))
        raw_tracks.append((name, events))
    tracks = []
    for name, events in raw_tracks:
        # split by channel: a track can hold several (type 0 files always do)
        by_ch: dict[int, dict] = {}
        open_notes: dict[tuple[int, int], list] = {}
        for t, kind, ch, d in sorted(events, key=lambda e: e[0]):
            tr = by_ch.setdefault(ch, {"name": name, "channel": ch, "notes": [], "cc": [], "bend": []})
            beat = t / tpq
            if kind == 0x90 and len(d) == 2 and d[1] > 0:
                open_notes.setdefault((ch, d[0]), []).append((beat, d[1]))
            elif (kind == 0x80 or (kind == 0x90 and len(d) == 2 and d[1] == 0)) and len(d) >= 1:
                stack = open_notes.get((ch, d[0]))
                if stack:
                    start, vel = stack.pop(0)
                    tr["notes"].append((start, max(1 / 64, beat - start), d[0], vel))
            elif kind == 0xB0 and len(d) == 2:
                tr["cc"].append((beat, d[0], d[1]))
            elif kind == 0xE0 and len(d) == 2:
                v = (d[0] | (d[1] << 7)) - 8192
                tr["bend"].append((beat, max(-1.0, min(1.0, v / 8192))))
        # notes never let go: until the last thing in the file
        end = max((t / tpq for t, *_ in events), default=0.0)
        for (ch, key), stack in open_notes.items():
            for start, vel in stack:
                by_ch[ch]["notes"].append((start, max(1 / 16, end - start), key, vel))
        for tr in by_ch.values():
            if tr["notes"] or tr["cc"] or tr["bend"]:
                tr["notes"].sort()
                tracks.append(tr)
    return {"tpq": tpq, "bpm": bpm, "beats": beats, "tracks": tracks}


def write(path: str | Path, tracks: list, bpm: float = 120.0, beats: int = 4, tpq: int = 480) -> None:
    """tracks: [{"name", "channel" (0-15; 9 is drums), "notes": [(start, len, key, vel)],
    optional "cc": [(beat, number, value)], "bend": [(beat, -1..1)]}]."""
    def chunk(body: bytes) -> bytes:
        return b"MTrk" + struct.pack(">I", len(body)) + body

    head = b"MThd" + struct.pack(">IHHH", 6, 1, len(tracks) + 1, tpq)
    us = int(round(60_000_000 / max(1.0, bpm)))
    meta = (b"\x00\xFF\x51\x03" + us.to_bytes(3, "big")
            + b"\x00\xFF\x58\x04" + bytes([max(1, min(32, int(beats))), 2, 24, 8]) + b"\x00\xFF\x2F\x00")
    out = [head, chunk(meta)]
    for tr in tracks:
        ch = int(tr.get("channel", 0)) & 15
        evs = []  # (tick, order, bytes): note offs and controller moves before ons at the same tick
        for start, ln, key, vel in tr.get("notes", []):
            a = int(round(float(start) * tpq))
            b = max(a + 1, int(round((float(start) + float(ln)) * tpq)))
            evs.append((a, 1, bytes([0x90 | ch, int(key) & 127, max(1, min(127, int(vel)))])))
            evs.append((b, 0, bytes([0x80 | ch, int(key) & 127, 0])))
        for beat, num, val in tr.get("cc", []):
            evs.append((int(round(float(beat) * tpq)), 0, bytes([0xB0 | ch, int(num) & 127, max(0, min(127, int(val)))])))
        for beat, v in tr.get("bend", []):
            raw = max(0, min(16383, int(round(float(v) * 8192)) + 8192))
            evs.append((int(round(float(beat) * tpq)), 0, bytes([0xE0 | ch, raw & 127, raw >> 7])))
        evs.sort(key=lambda e: (e[0], e[1]))
        name = str(tr.get("name") or "")[:60].encode("latin-1", "replace")
        body = bytearray(b"\x00\xFF\x03" + _vlq_bytes(len(name)) + name)
        last = 0
        for tick, _, msg in evs:
            body += _vlq_bytes(tick - last) + msg
            last = tick
        body += b"\x00\xFF\x2F\x00"
        out.append(chunk(bytes(body)))
    Path(path).write_bytes(b"".join(out))
