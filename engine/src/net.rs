//! Wire format, connect codes and STUN. Byte-compatible with the Python
//! engine (net.py) so old and new builds can still talk to each other.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};

pub const MAGIC: &[u8; 2] = b"DU";
pub const HEADER: usize = 7; // magic(2) type(1) seq(4)

pub const T_HELLO: u8 = 1; // payload: samplerate u32, frame u16
pub const T_AUDIO: u8 = 2; // payload: i16 LE mono PCM
pub const T_PING: u8 = 3; // payload: f64 time
pub const T_PONG: u8 = 4;
pub const T_BYE: u8 = 5;
pub const T_SIGNAL: u8 = 6; // seq = message id; payload: chunk idx u16, count u16, bytes
pub const T_SIGACK: u8 = 7;
/// Multichannel audio (protocol 2). payload: total channels u8, first channel
/// u8, channels in this packet u8, then i16 LE samples, channel after channel.
/// A block with more channels than fit under the MTU is split over packets.
pub const T_AUDIO_MC: u8 = 8;
/// File transfer (transfer.rs): seq = transfer id. Older builds ignore both.
pub const T_FILE: u8 = 9;
pub const T_FILEACK: u8 = 10;
/// Notes played live on a song track (livenotes.rs): seq = event number.
pub const T_NOTE: u8 = 11;

/// Sent as an extra byte after HELLO's samplerate/frame. Builds that predate it
/// send 6 bytes, so a missing byte means version 1: mono T_AUDIO only.
pub const PROTOCOL: u8 = 2;
pub const MAX_AUDIO_PAYLOAD: usize = 1200;

pub const SIGNAL_CHUNK: usize = 1000;
pub const STUN_MAGIC: u32 = 0x2112_A442;
pub const STUN_SERVERS: [&str; 3] = [
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

pub fn pack(ptype: u8, seq: u32, payload: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(MAGIC);
    out.push(ptype);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(payload);
}

/// (type, seq, payload) if this is one of our packets.
pub fn unpack(data: &[u8]) -> Option<(u8, u32, &[u8])> {
    if data.len() < HEADER || &data[..2] != MAGIC {
        return None;
    }
    let seq = u32::from_be_bytes([data[3], data[4], data[5], data[6]]);
    Some((data[2], seq, &data[HEADER..]))
}

// --- connect codes -----------------------------------------------------------

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn b32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn b32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = B32.iter().position(|&x| x == c)? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

pub fn make_code(public: Option<SocketAddrV4>, local: SocketAddrV4) -> String {
    let public = public.unwrap_or(local);
    let mut raw = Vec::with_capacity(12);
    for a in [public, local] {
        raw.extend_from_slice(&a.ip().octets());
        raw.extend_from_slice(&a.port().to_be_bytes());
    }
    let s = b32_encode(&raw);
    s.as_bytes().chunks(5).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
}

/// Candidate addresses from a code, or from a plain "host:port".
pub fn parse_code(code: &str) -> Result<Vec<SocketAddrV4>, String> {
    let code = code.trim();
    if code.contains(':') {
        let addr = code
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .find_map(|a| match a {
                SocketAddr::V4(v4) => Some(v4),
                _ => None,
            })
            .ok_or("no IPv4 address")?;
        return Ok(vec![addr]);
    }
    let cleaned: String = code.chars().filter(|c| *c != '-' && *c != ' ').collect::<String>().to_uppercase();
    let raw = b32_decode(&cleaned).ok_or("bad characters")?;
    if raw.len() != 12 {
        return Err("code has the wrong length".into());
    }
    let addr = |b: &[u8]| SocketAddrV4::new(Ipv4Addr::new(b[0], b[1], b[2], b[3]), u16::from_be_bytes([b[4], b[5]]));
    let (public, local) = (addr(&raw[0..6]), addr(&raw[6..12]));
    Ok(if public == local { vec![public] } else { vec![public, local] })
}

pub fn local_ip() -> Ipv4Addr {
    // "connect" a UDP socket to learn which interface routes out; sends nothing
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .ok()
        .and_then(|a| match a {
            SocketAddr::V4(v4) => Some(*v4.ip()),
            _ => None,
        })
        .unwrap_or(Ipv4Addr::LOCALHOST)
}

// --- STUN ----------------------------------------------------------------------

pub fn stun_request(txid: &[u8; 12]) -> Vec<u8> {
    let mut v = Vec::with_capacity(20);
    v.extend_from_slice(&0x0001u16.to_be_bytes());
    v.extend_from_slice(&0u16.to_be_bytes());
    v.extend_from_slice(&STUN_MAGIC.to_be_bytes());
    v.extend_from_slice(txid);
    v
}

pub fn is_stun_response(data: &[u8]) -> bool {
    data.len() >= 20 && data[0..2] == [0x01, 0x01] && data[4..8] == STUN_MAGIC.to_be_bytes()
}

pub fn parse_stun_response(data: &[u8]) -> Option<SocketAddrV4> {
    if !is_stun_response(data) {
        return None;
    }
    let len = u16::from_be_bytes([data[2], data[3]]) as usize;
    let end = (20 + len).min(data.len());
    let mut pos = 20;
    let mut mapped = None;
    while pos + 4 <= end {
        let atype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let alen = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        let val = &data[pos + 4..(pos + 4 + alen).min(end)];
        if val.len() >= 8 && val[1] == 0x01 {
            let port = u16::from_be_bytes([val[2], val[3]]);
            let ip = u32::from_be_bytes([val[4], val[5], val[6], val[7]]);
            if atype == 0x0020 || atype == 0x8020 {
                return Some(SocketAddrV4::new(
                    Ipv4Addr::from(ip ^ STUN_MAGIC),
                    port ^ (STUN_MAGIC >> 16) as u16,
                ));
            }
            if atype == 0x0001 {
                mapped = Some(SocketAddrV4::new(Ipv4Addr::from(ip), port));
            }
        }
        pos += 4 + alen + ((4 - alen % 4) % 4);
    }
    mapped
}

pub fn resolve(host: &str) -> Option<SocketAddrV4> {
    host.to_socket_addrs().ok()?.find_map(|a| match a {
        SocketAddr::V4(v4) => Some(v4),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_roundtrip_matches_python() {
        // same addresses as selftest.py's example; Python produced ZMAHC-B6DZP-AKQAI-UYPFQ
        let code = make_code(Some("203.0.113.7:50123".parse().unwrap()), "192.168.1.20:50123".parse().unwrap());
        assert_eq!(code, "ZMAHC-B6DZP-AKQAI-UYPFQ");
        let back = parse_code(&code.to_lowercase()).unwrap();
        assert_eq!(back, vec!["203.0.113.7:50123".parse().unwrap(), "192.168.1.20:50123".parse().unwrap()]);
    }
}
