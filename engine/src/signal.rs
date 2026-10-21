//! Reliable side channel for small messages (WebRTC video setup): chunked,
//! resent every 300 ms until the partner acks the whole message. Not realtime;
//! lives behind a mutex on the network threads.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::net;

pub struct Signals {
    next_id: u32,
    outbox: HashMap<u32, (Vec<Vec<u8>>, Instant, u32)>, // packets, next send, tries
    partial: HashMap<u32, HashMap<u16, Vec<u8>>>,
    done: HashMap<u32, Instant>,
    pub inbox: Vec<String>,
}

impl Signals {
    pub fn new() -> Self {
        Signals {
            next_id: rand::random::<u32>() & 0x7FFF_FFFF,
            outbox: HashMap::new(),
            partial: HashMap::new(),
            done: HashMap::new(),
            inbox: Vec::new(),
        }
    }

    pub fn queue(&mut self, text: &str) {
        let data = text.as_bytes();
        let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(net::SIGNAL_CHUNK).collect() };
        let id = self.next_id;
        self.next_id = (self.next_id + 1) & 0x7FFF_FFFF;
        let count = chunks.len() as u16;
        let packets = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut payload = Vec::with_capacity(4 + c.len());
                payload.extend_from_slice(&(i as u16).to_be_bytes());
                payload.extend_from_slice(&count.to_be_bytes());
                payload.extend_from_slice(c);
                let mut p = Vec::new();
                net::pack(net::T_SIGNAL, id, &payload, &mut p);
                p
            })
            .collect();
        self.outbox.insert(id, (packets, Instant::now(), 0));
    }

    /// Packets due for (re)sending now.
    pub fn due(&mut self) -> Vec<Vec<u8>> {
        let now = Instant::now();
        let mut out = Vec::new();
        self.outbox.retain(|_, (packets, next, tries)| {
            if *tries >= 30 {
                return false; // ~9 s without an ack: give up
            }
            if now >= *next {
                *next = now + Duration::from_millis(300);
                *tries += 1;
                out.extend(packets.iter().cloned());
            }
            true
        });
        out
    }

    pub fn pending(&self) -> usize {
        self.outbox.len()
    }

    pub fn ack(&mut self, id: u32) {
        self.outbox.remove(&id);
    }

    /// Returns true when the message is complete (caller sends an ack).
    pub fn on_chunk(&mut self, id: u32, body: &[u8]) -> bool {
        if body.len() < 4 {
            return false;
        }
        let idx = u16::from_be_bytes([body[0], body[1]]);
        let count = u16::from_be_bytes([body[2], body[3]]);
        if !self.done.contains_key(&id) {
            let parts = self.partial.entry(id).or_default();
            parts.insert(idx, body[4..].to_vec());
            if parts.len() == count as usize {
                let parts = self.partial.remove(&id).unwrap();
                let mut bytes = Vec::new();
                for i in 0..count {
                    bytes.extend_from_slice(&parts[&i]);
                }
                self.inbox.push(String::from_utf8_lossy(&bytes).into_owned());
                self.done.insert(id, Instant::now());
            }
        }
        if self.done.len() > 512 {
            let cutoff = Instant::now() - Duration::from_secs(60);
            self.done.retain(|_, t| *t > cutoff);
        }
        self.done.contains_key(&id)
    }

    pub fn reset(&mut self) {
        self.outbox.clear();
        self.partial.clear();
    }
}
