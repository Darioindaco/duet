//! Reliable file transfer to the partner over the audio socket, for recorded
//! takes. Not realtime: lives behind its own mutex, ticked by a small thread.
//!
//! The sender paces chunks at a rate it adapts like TCP does (a little faster
//! while everything arrives, back off when chunks go missing), so a big take
//! never swamps the uplink the live audio needs. The receiver acks with a
//! cumulative count plus a bitmap of what it has beyond that; the sender
//! resends what stays unacked for too long. One file goes out at a time.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::net;

pub const CHUNK: usize = 1100;
const ACK_BITS: usize = 1024; // chunks beyond the cumulative count an ack covers
const MAX_INFLIGHT: u32 = ACK_BITS as u32;
const MIN_RATE: f64 = 64_000.0; // bytes/s
const START_RATE: f64 = 250_000.0;
const MAX_RATE: f64 = 1_500_000.0; // ~12 Mbit/s: plenty for takes, leaves room for the rest
const STEP_UP: f64 = 40_000.0; // per clean 100 ms
const META_EVERY: Duration = Duration::from_millis(300);
const GIVE_UP: Duration = Duration::from_secs(20); // nothing acked for this long

// payload kinds
const K_META: u8 = 0; // T_FILE: size u64, name; T_FILEACK: got the meta
const K_DATA: u8 = 1; // T_FILE: index u32, bytes; T_FILEACK: cumulative u32, bitmap
const K_DONE: u8 = 2; // T_FILEACK: all there and saved

struct Outgoing {
    id: u32,
    name: String,
    data: Vec<u8>,
    chunks: u32,
    acked: Vec<bool>,
    n_acked: u32,
    sent_at: Vec<Option<Instant>>,
    next_new: u32,
    meta_acked: bool,
    meta_sent: Option<Instant>,
    last_progress: Instant,
}

struct Incoming {
    name: String,
    size: usize,
    got: Vec<bool>,
    n_got: u32,
    data: Vec<u8>,
    unacked: u32,
    last_ack: Instant,
}

pub struct Progress {
    pub name: String,
    pub done: u64,
    pub total: u64,
}

pub struct Transfers {
    dir: PathBuf,
    queue: VecDeque<Outgoing>,
    incoming: HashMap<u32, Incoming>,
    finished_ids: HashMap<u32, Instant>,
    /// received and saved: (name, path)
    pub received: Vec<(String, PathBuf)>,
    /// delivered to the partner
    pub sent: Vec<String>,
    /// gave up on (partner gone, or the file couldn't be saved there)
    pub failed: Vec<String>,
    rate: f64,
    tokens: f64,
    last_tick: Instant,
    last_cut: Instant,
    last_raise: Instant,
    loss_since_raise: bool,
}

fn n_chunks(size: usize) -> u32 {
    size.div_ceil(CHUNK).max(1) as u32
}

/// Keep incoming names to plain file names: no paths from the other side.
fn safe_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).collect();
    let s = s.trim_start_matches('.').to_string();
    if s.is_empty() { "file".into() } else { s }
}

impl Transfers {
    pub fn new(dir: PathBuf) -> Self {
        let now = Instant::now();
        Transfers {
            dir,
            queue: VecDeque::new(),
            incoming: HashMap::new(),
            finished_ids: HashMap::new(),
            received: vec![],
            sent: vec![],
            failed: vec![],
            rate: START_RATE,
            tokens: 0.0,
            last_tick: now,
            last_cut: now,
            last_raise: now,
            loss_since_raise: false,
        }
    }

    pub fn set_dir(&mut self, dir: PathBuf) {
        self.dir = dir;
    }

    pub fn active(&self) -> bool {
        !self.queue.is_empty() || !self.incoming.is_empty()
    }

    pub fn send(&mut self, name: &str, data: Vec<u8>) {
        if self.queue.iter().any(|o| o.name == name) {
            return; // already on its way
        }
        let chunks = n_chunks(data.len());
        self.queue.push_back(Outgoing {
            id: rand::random::<u32>(),
            name: name.to_string(),
            data,
            chunks,
            acked: vec![false; chunks as usize],
            n_acked: 0,
            sent_at: vec![None; chunks as usize],
            next_new: 0,
            meta_acked: false,
            meta_sent: None,
            last_progress: Instant::now(),
        });
    }

    /// The partner went away: everything in flight is off.
    pub fn abort_all(&mut self) {
        for o in self.queue.drain(..) {
            self.failed.push(o.name);
        }
        self.incoming.clear();
    }

    pub fn progress(&self) -> (Vec<Progress>, Vec<Progress>) {
        let out = self
            .queue
            .iter()
            .map(|o| Progress { name: o.name.clone(), done: (o.n_acked as u64 * CHUNK as u64).min(o.data.len() as u64), total: o.data.len() as u64 })
            .collect();
        let inc = self
            .incoming
            .values()
            .map(|i| Progress { name: i.name.clone(), done: (i.n_got as u64 * CHUNK as u64).min(i.size as u64), total: i.size as u64 })
            .collect();
        (out, inc)
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Packets to send now. `rtt` is the current ping.
    pub fn tick(&mut self, rtt: Option<f64>) -> Vec<Vec<u8>> {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        let rtt = Duration::from_secs_f64(rtt.unwrap_or(100.0).clamp(1.0, 2000.0) / 1000.0);
        let rto = (rtt * 2 + Duration::from_millis(60)).max(Duration::from_millis(150));
        let mut out = Vec::new();

        // receiver side: acks for whatever came in since the last one
        for (id, inc) in self.incoming.iter_mut() {
            if inc.unacked > 0 && now.duration_since(inc.last_ack) >= Duration::from_millis(15) {
                out.push(ack_packet(*id, inc));
                inc.unacked = 0;
                inc.last_ack = now;
            }
        }
        let cutoff = now - Duration::from_secs(120);
        self.finished_ids.retain(|_, t| *t > cutoff);

        // sender side
        if now.duration_since(self.last_raise) >= Duration::from_millis(100) {
            if !self.loss_since_raise {
                self.rate = (self.rate + STEP_UP).min(MAX_RATE);
            }
            self.loss_since_raise = false;
            self.last_raise = now;
        }
        self.tokens = (self.tokens + self.rate * dt).min(self.rate * 0.02 + 2.0 * CHUNK as f64);
        let Some(o) = self.queue.front_mut() else { return out };
        if now.duration_since(o.last_progress) > GIVE_UP {
            let o = self.queue.pop_front().unwrap();
            self.failed.push(o.name);
            return out;
        }
        if !o.meta_acked {
            if o.meta_sent.is_none_or(|t| now.duration_since(t) >= META_EVERY) {
                let mut body = vec![K_META];
                body.extend_from_slice(&(o.data.len() as u64).to_be_bytes());
                body.extend_from_slice(o.name.as_bytes());
                let mut p = Vec::new();
                net::pack(net::T_FILE, o.id, &body, &mut p);
                out.push(p);
                o.meta_sent = Some(now);
            }
            return out;
        }
        let mut lost = false;
        // resends first: oldest unacked chunks past their timeout
        let base = o.acked.iter().position(|a| !a).unwrap_or(o.chunks as usize) as u32;
        let mut i = base;
        while i < o.next_new && self.tokens >= CHUNK as f64 {
            let k = i as usize;
            if !o.acked[k] && o.sent_at[k].is_some_and(|t| now.duration_since(t) >= rto) {
                out.push(data_packet(o, i));
                o.sent_at[k] = Some(now);
                self.tokens -= (CHUNK + 16) as f64;
                lost = true;
            }
            i += 1;
        }
        // then new ones, within the window the acks can describe
        while o.next_new < o.chunks && o.next_new < base + MAX_INFLIGHT && self.tokens >= CHUNK as f64 {
            out.push(data_packet(o, o.next_new));
            o.sent_at[o.next_new as usize] = Some(now);
            o.next_new += 1;
            self.tokens -= (CHUNK + 16) as f64;
        }
        if lost {
            self.loss_since_raise = true;
            if now.duration_since(self.last_cut) >= rtt.max(Duration::from_millis(50)) {
                self.rate = (self.rate * 0.7).max(MIN_RATE);
                self.last_cut = now;
            }
        }
        out
    }

    /// A T_FILE packet arrived. Returns a reply to send right away, if any.
    pub fn on_file(&mut self, id: u32, body: &[u8]) -> Option<Vec<u8>> {
        let (&kind, rest) = body.split_first()?;
        if self.finished_ids.contains_key(&id) {
            return Some(done_packet(id)); // our DONE got lost: say it again
        }
        match kind {
            K_META if rest.len() >= 8 => {
                let size = u64::from_be_bytes(rest[..8].try_into().unwrap()) as usize;
                if size > 2_000_000_000 {
                    return None;
                }
                let name = safe_name(&String::from_utf8_lossy(&rest[8..]));
                let chunks = n_chunks(size);
                self.incoming.entry(id).or_insert_with(|| Incoming {
                    name,
                    size,
                    got: vec![false; chunks as usize],
                    n_got: 0,
                    data: vec![0; size],
                    unacked: 0,
                    last_ack: Instant::now(),
                });
                let mut p = Vec::new();
                net::pack(net::T_FILEACK, id, &[K_META], &mut p);
                Some(p)
            }
            K_DATA if rest.len() >= 4 => {
                let idx = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
                let bytes = &rest[4..];
                let inc = self.incoming.get_mut(&id)?;
                if idx >= inc.got.len() || inc.got[idx] {
                    inc.unacked += 1;
                    return None;
                }
                let at = idx * CHUNK;
                let end = (at + bytes.len()).min(inc.size);
                inc.data[at..end].copy_from_slice(&bytes[..end - at]);
                inc.got[idx] = true;
                inc.n_got += 1;
                inc.unacked += 1;
                if inc.n_got as usize == inc.got.len() {
                    let inc = self.incoming.remove(&id).unwrap();
                    self.finish(id, inc);
                    return Some(done_packet(id));
                }
                if inc.unacked >= 16 {
                    inc.unacked = 0;
                    inc.last_ack = Instant::now();
                    return Some(ack_packet(id, inc));
                }
                None
            }
            _ => None,
        }
    }

    fn finish(&mut self, id: u32, inc: Incoming) {
        self.finished_ids.insert(id, Instant::now());
        let _ = std::fs::create_dir_all(&self.dir);
        let path = self.dir.join(&inc.name);
        let part = self.dir.join(format!("{}.part", inc.name));
        match std::fs::write(&part, &inc.data).and_then(|_| std::fs::rename(&part, &path)) {
            Ok(()) => self.received.push((inc.name, path)),
            Err(_) => self.failed.push(inc.name),
        }
    }

    /// A T_FILEACK packet arrived.
    pub fn on_ack(&mut self, id: u32, body: &[u8]) {
        let Some(pos) = self.queue.iter().position(|o| o.id == id) else { return };
        let o = &mut self.queue[pos];
        match body.first() {
            Some(&K_META) => {
                if !o.meta_acked {
                    o.meta_acked = true;
                    o.last_progress = Instant::now();
                }
            }
            Some(&K_DATA) if body.len() >= 5 => {
                o.meta_acked = true;
                let cum = (u32::from_be_bytes(body[1..5].try_into().unwrap())).min(o.chunks);
                let before = o.n_acked;
                for k in 0..cum as usize {
                    if !o.acked[k] {
                        o.acked[k] = true;
                        o.n_acked += 1;
                    }
                }
                for (byte_i, b) in body[5..].iter().enumerate() {
                    for bit in 0..8 {
                        let k = cum as usize + byte_i * 8 + bit;
                        if b & (1 << bit) != 0 && k < o.acked.len() && !o.acked[k] {
                            o.acked[k] = true;
                            o.n_acked += 1;
                        }
                    }
                }
                if o.n_acked > before {
                    o.last_progress = Instant::now();
                }
            }
            Some(&K_DONE) => {
                let o = self.queue.remove(pos).unwrap();
                self.sent.push(o.name);
            }
            _ => {}
        }
    }
}

fn data_packet(o: &Outgoing, i: u32) -> Vec<u8> {
    let at = i as usize * CHUNK;
    let end = (at + CHUNK).min(o.data.len());
    let mut body = Vec::with_capacity(5 + end - at);
    body.push(K_DATA);
    body.extend_from_slice(&i.to_be_bytes());
    body.extend_from_slice(&o.data[at..end]);
    let mut p = Vec::new();
    net::pack(net::T_FILE, o.id, &body, &mut p);
    p
}

fn ack_packet(id: u32, inc: &Incoming) -> Vec<u8> {
    let cum = inc.got.iter().position(|g| !g).unwrap_or(inc.got.len());
    let mut body = vec![K_DATA];
    body.extend_from_slice(&(cum as u32).to_be_bytes());
    let mut bits = vec![0u8; ACK_BITS / 8];
    for k in 0..ACK_BITS {
        if inc.got.get(cum + k).copied().unwrap_or(false) {
            bits[k / 8] |= 1 << (k % 8);
        }
    }
    // trailing zero bytes say nothing: leave them off
    while bits.last() == Some(&0) {
        bits.pop();
    }
    body.extend_from_slice(&bits);
    let mut p = Vec::new();
    net::pack(net::T_FILEACK, id, &body, &mut p);
    p
}

fn done_packet(id: u32) -> Vec<u8> {
    let mut p = Vec::new();
    net::pack(net::T_FILEACK, id, &[K_DONE], &mut p);
    p
}

pub fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two endpoints and a lossy, reordering link between them.
    fn run(size: usize, loss: f32, max_ticks: usize) -> (Transfers, Transfers, usize) {
        let dir = std::env::temp_dir().join(format!("duet-xfer-{}-{}", std::process::id(), size));
        let mut a = Transfers::new(dir.clone());
        let mut b = Transfers::new(dir);
        let data: Vec<u8> = (0..size).map(|i| (i * 7 + i / 300) as u8).collect();
        a.send("take-1.wav", data);
        let mut rng = 12345u32;
        let mut coin = move || {
            rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
            (rng >> 16) as f32 / 65536.0
        };
        let mut ticks = 0;
        let mut to_b: Vec<Vec<u8>> = vec![];
        let mut to_a: Vec<Vec<u8>> = vec![];
        while ticks < max_ticks && a.sent.is_empty() {
            ticks += 1;
            std::thread::sleep(Duration::from_millis(2));
            to_b.extend(a.tick(Some(20.0)));
            to_a.extend(b.tick(Some(20.0)));
            // deliver, dropping some and swapping neighbours now and then
            if to_b.len() > 1 && coin() < 0.2 {
                to_b.swap(0, 1);
            }
            for p in std::mem::take(&mut to_b) {
                if coin() < loss {
                    continue;
                }
                let (_, id, body) = net::unpack(&p).unwrap();
                if let Some(r) = b.on_file(id, body) {
                    to_a.push(r);
                }
            }
            for p in std::mem::take(&mut to_a) {
                if coin() < loss {
                    continue;
                }
                let (_, id, body) = net::unpack(&p).unwrap();
                a.on_ack(id, body);
            }
        }
        (a, b, ticks)
    }

    #[test]
    fn a_file_arrives_whole_over_a_lossy_link() {
        let size = 300_000;
        let (a, b, _) = run(size, 0.1, 20_000);
        assert_eq!(a.sent, vec!["take-1.wav".to_string()]);
        let (name, path) = &b.received[0];
        assert_eq!(name, "take-1.wav");
        let got = std::fs::read(path).unwrap();
        assert_eq!(got.len(), size);
        assert!(got.iter().enumerate().all(|(i, v)| *v == (i * 7 + i / 300) as u8));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn empty_and_odd_sizes_work_and_names_stay_plain() {
        let (a, b, _) = run(0, 0.0, 2000);
        assert_eq!(a.sent.len(), 1);
        assert_eq!(std::fs::read(&b.received[0].1).unwrap().len(), 0);
        assert_eq!(safe_name("../../etc/passwd"), "etcpasswd");
        assert_eq!(safe_name("take 1.wav"), "take1.wav");
    }
}
