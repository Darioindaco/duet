"""Networking helpers: STUN address discovery, connect codes, packet format.

Nothing here talks to a server we run. STUN is a free public service that just
tells us "this is the address your router shows to the internet"; after that the
two peers send UDP straight to each other (UDP hole punching).
"""
from __future__ import annotations

import base64
import os
import socket
import struct

# Public STUN servers, tried in order. All free, no account.
STUN_SERVERS = [
    ("stun.l.google.com", 19302),
    ("stun1.l.google.com", 19302),
    ("stun.cloudflare.com", 3478),
]

STUN_MAGIC = 0x2112A442

# --- packet format -----------------------------------------------------------
# header: magic "DU" | type (u8) | seq (u32)
MAGIC = b"DU"
HEADER = struct.Struct("!2sBI")
T_HELLO = 1   # hole punching / handshake, payload: samplerate (u32), frame (u16)
T_AUDIO = 2   # payload: int16 mono PCM, `frame` samples
T_PING = 3    # payload: sender's monotonic time (f64)
T_PONG = 4    # payload: echoed time (f64)
T_BYE = 5
T_SIGNAL = 6  # reliable side channel (WebRTC video setup), seq = message id
              # payload: chunk index (u16), chunk count (u16), utf-8 bytes
T_SIGACK = 7  # seq = id of a fully received message

HELLO_BODY = struct.Struct("!IH")
CHUNK_HEAD = struct.Struct("!HH")
SIGNAL_CHUNK = 1000  # bytes per datagram: stays under any sane MTU, no IP fragmentation
TIME_BODY = struct.Struct("!d")


def pack(ptype: int, seq: int, payload: bytes = b"") -> bytes:
    return HEADER.pack(MAGIC, ptype, seq & 0xFFFFFFFF) + payload


def unpack(data: bytes):
    """Return (type, seq, payload) or None if this isn't one of our packets."""
    if len(data) < HEADER.size or data[:2] != MAGIC:
        return None
    _, ptype, seq = HEADER.unpack_from(data)
    return ptype, seq, data[HEADER.size:]


# --- address discovery -------------------------------------------------------

def local_ip() -> str:
    """LAN IP of the interface that routes to the internet (sends no traffic)."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("8.8.8.8", 80))
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


def stun_request() -> tuple[bytes, bytes]:
    """(transaction id, binding request packet)."""
    txid = os.urandom(12)
    return txid, struct.pack("!HHI", 0x0001, 0, STUN_MAGIC) + txid


def is_stun_response(data: bytes) -> bool:
    return len(data) >= 20 and data[0:2] == b"\x01\x01" and data[4:8] == struct.pack("!I", STUN_MAGIC)


def resolve_stun_servers():
    for host, port in STUN_SERVERS:
        try:
            yield socket.getaddrinfo(host, port, socket.AF_INET, socket.SOCK_DGRAM)[0][4]
        except OSError:
            continue


def parse_stun_response(data: bytes):
    msg_type, msg_len = struct.unpack_from("!HH", data)
    if msg_type != 0x0101:
        return None
    pos, end = 20, min(len(data), 20 + msg_len)
    mapped = None
    while pos + 4 <= end:
        atype, alen = struct.unpack_from("!HH", data, pos)
        val = data[pos + 4: pos + 4 + alen]
        if atype in (0x0020, 0x8020) and len(val) >= 8 and val[1] == 0x01:  # XOR-MAPPED-ADDRESS, IPv4
            port = struct.unpack_from("!H", val, 2)[0] ^ (STUN_MAGIC >> 16)
            ip_int = struct.unpack_from("!I", val, 4)[0] ^ STUN_MAGIC
            return socket.inet_ntoa(struct.pack("!I", ip_int)), port
        if atype == 0x0001 and len(val) >= 8 and val[1] == 0x01:  # MAPPED-ADDRESS fallback
            mapped = socket.inet_ntoa(val[4:8]), struct.unpack_from("!H", val, 2)[0]
        pos += 4 + alen + (-alen % 4)
    return mapped


# --- connect codes -----------------------------------------------------------
# A code packs the public and LAN candidates: 2 x (ipv4 + port) = 12 bytes,
# which base32-encodes to a 20-character string you can paste in a chat.

def make_code(public, local) -> str:
    pub = public or local
    raw = (socket.inet_aton(pub[0]) + struct.pack("!H", pub[1])
           + socket.inet_aton(local[0]) + struct.pack("!H", local[1]))
    s = base64.b32encode(raw).decode().rstrip("=")
    return "-".join(s[i:i + 5] for i in range(0, len(s), 5))


def parse_code(code: str):
    """Return a list of candidate (ip, port) addresses.

    Also accepts a plain "ip:port" for manual/port-forwarded setups.
    """
    code = code.strip()
    if ":" in code:
        host, port = code.rsplit(":", 1)
        return [(socket.gethostbyname(host.strip()), int(port))]
    s = code.replace("-", "").replace(" ", "").upper()
    raw = base64.b32decode(s + "=" * (-len(s) % 8))
    if len(raw) != 12:
        raise ValueError("code has the wrong length")
    pub = socket.inet_ntoa(raw[0:4]), struct.unpack("!H", raw[4:6])[0]
    loc = socket.inet_ntoa(raw[6:10]), struct.unpack("!H", raw[10:12])[0]
    return [pub] if pub == loc else [pub, loc]
