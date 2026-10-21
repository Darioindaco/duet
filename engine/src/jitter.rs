//! Jitter buffer: reorders incoming frames and plays them out at a fixed depth.
//! Same algorithm as the original Python version, with preallocated slots so
//! the audio thread never allocates.
//!
//! Multichannel: one slot = one block for all of the partner's channels. A
//! block can arrive split over several packets (channel groups, to stay under
//! the MTU); a slot plays as soon as its turn comes, with any channel whose
//! packet didn't make it left silent.

pub const MAX_FRAME: usize = 1024;
pub const MAX_CH: usize = 8;
const SLOTS: usize = 64;
const TRIM_WINDOW: u32 = 200; // blocks to watch before trimming drift
const HARD_TRIM: i64 = 32; // frames over target that get trimmed right away
const RESTART_GAP: i64 = 5000;
const STARVE_RESET: u32 = 400;

struct Slot {
    seq: i64,
    channels: usize,
    len: usize,
    mask: u32, // which channels arrived
    data: Vec<f32>, // channel-major, stride MAX_FRAME
}

#[derive(Default, Clone, Copy)]
pub struct Stats {
    pub underruns: u64,
    pub lost: u64,
    pub late: u64,
    pub dropped: u64,
}

pub struct JitterBuffer {
    pub target: i64,
    slots: Vec<Slot>,
    next: Option<i64>,
    highest: i64,
    started: bool,
    starved: u32,
    win_min: i64,
    win_count: u32,
    pub stats: Stats,
}

impl JitterBuffer {
    pub fn new(target: i64) -> Self {
        let slots = (0..SLOTS)
            .map(|_| Slot { seq: -1, channels: 0, len: 0, mask: 0, data: vec![0.0; MAX_CH * MAX_FRAME] })
            .collect();
        let mut jb = JitterBuffer {
            target,
            slots,
            next: None,
            highest: -1,
            started: false,
            starved: 0,
            win_min: i64::MAX,
            win_count: 0,
            stats: Stats::default(),
        };
        jb.reset();
        jb
    }

    pub fn reset(&mut self) {
        for s in self.slots.iter_mut() {
            s.seq = -1;
        }
        self.next = None;
        self.highest = -1;
        self.started = false;
        self.starved = 0;
        self.win_min = i64::MAX;
        self.win_count = 0;
    }

    pub fn depth(&self) -> i64 {
        match self.next {
            None => self.slots.iter().filter(|s| s.seq >= 0).count() as i64,
            Some(n) => (self.highest - n + 1).max(0),
        }
    }

    fn index(seq: i64) -> usize {
        (seq as usize) % SLOTS
    }

    fn min_seq(&self) -> Option<i64> {
        self.slots.iter().filter(|s| s.seq >= 0).map(|s| s.seq).min()
    }

    /// One packet: `count` channels starting at `first`, of a `total`-channel
    /// block, `len` samples each, channel-major in `samples`.
    pub fn push(&mut self, seq: u32, total: usize, first: usize, count: usize, len: usize, samples: &[f32]) {
        let seq = seq as i64;
        let total = total.clamp(1, MAX_CH);
        let len = len.min(MAX_FRAME);
        if let Some(n) = self.next {
            if (seq - n).abs() > RESTART_GAP {
                self.reset(); // partner restarted their audio: start over
            }
        }
        if let Some(n) = self.next {
            if seq < n {
                self.stats.late += 1;
                return;
            }
        }
        let slot = &mut self.slots[Self::index(seq)];
        if slot.seq != seq {
            slot.seq = seq;
            slot.channels = total;
            slot.len = len;
            slot.mask = 0;
        }
        for c in 0..count {
            let ch = first + c;
            if ch >= total || (c + 1) * len > samples.len() {
                break;
            }
            slot.data[ch * MAX_FRAME..ch * MAX_FRAME + len].copy_from_slice(&samples[c * len..(c + 1) * len]);
            slot.mask |= 1 << ch;
        }
        if seq > self.highest {
            self.highest = seq;
        }
    }

    /// Next block into `out` (channel-major, stride MAX_FRAME, `n` samples per
    /// channel). Returns the channel count, or None when there's nothing to play.
    pub fn pop(&mut self, out: &mut [f32], n: usize) -> Option<usize> {
        if !self.started {
            let first = self.min_seq()?;
            if self.highest - first + 1 < self.target {
                return None;
            }
            self.next = Some(first.max(self.highest - self.target + 1));
            self.started = true;
            self.starved = 0;
        }
        let mut next = self.next.unwrap();

        // Drivers deliver in bursts, so only trim when even the lowest depth over
        // ~half a second sits above target, or right away after a pile-up.
        let depth = self.highest - next + 1;
        self.win_min = self.win_min.min(depth);
        self.win_count += 1;
        let mut excess = 0;
        if depth > self.target + HARD_TRIM {
            excess = depth - self.target;
        } else if self.win_count >= TRIM_WINDOW {
            if self.win_min > self.target + 1 {
                excess = self.win_min - self.target;
            }
            self.win_min = i64::MAX;
            self.win_count = 0;
        }
        if excess > 0 {
            for s in next..next + excess {
                let slot = &mut self.slots[Self::index(s)];
                if slot.seq == s {
                    slot.seq = -1;
                }
            }
            next += excess;
            self.stats.dropped += excess as u64;
            self.win_min = i64::MAX;
            self.win_count = 0;
        }

        let slot = &mut self.slots[Self::index(next)];
        if slot.seq == next {
            slot.seq = -1;
            let len = slot.len.min(n);
            for ch in 0..slot.channels {
                let dst = &mut out[ch * MAX_FRAME..ch * MAX_FRAME + n];
                if slot.mask & (1 << ch) != 0 {
                    dst[..len].copy_from_slice(&slot.data[ch * MAX_FRAME..ch * MAX_FRAME + len]);
                    dst[len..].fill(0.0);
                } else {
                    dst.fill(0.0); // that channel's packet got lost
                }
            }
            self.next = Some(next + 1);
            self.starved = 0;
            return Some(slot.channels);
        }
        if self.highest > next {
            // later frames are here, so this one is lost: move on
            self.stats.lost += 1;
            self.next = Some(next + 1);
            return None;
        }
        // Ran dry. Don't advance, so a merely late frame still plays (this also
        // re-adds a frame of delay, which corrects for a slow partner clock).
        self.next = Some(next);
        self.stats.underruns += 1;
        self.starved += 1;
        if self.starved > STARVE_RESET {
            self.reset();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push1(jb: &mut JitterBuffer, s: u32, v: f32) {
        jb.push(s, 1, 0, 1, 128, &[v; 128]);
    }
    fn pop(jb: &mut JitterBuffer) -> Option<f32> {
        let mut out = vec![0f32; MAX_CH * MAX_FRAME];
        jb.pop(&mut out, 128).map(|_| out[0])
    }

    // mirrors the original selftest.py jitter test step by step
    #[test]
    fn behaves_like_python_version() {
        let mut jb = JitterBuffer::new(3);
        assert_eq!(pop(&mut jb), None);
        for s in [1, 0, 2] {
            push1(&mut jb, s, s as f32);
        }
        assert_eq!([pop(&mut jb), pop(&mut jb), pop(&mut jb)], [Some(0.), Some(1.), Some(2.)]);
        assert_eq!(pop(&mut jb), None);
        assert_eq!(jb.stats.underruns, 1);
        push1(&mut jb, 3, 3.);
        assert_eq!(pop(&mut jb), Some(3.));
        push1(&mut jb, 5, 5.);
        assert_eq!(pop(&mut jb), None);
        assert_eq!(jb.stats.lost, 1);
        assert_eq!(pop(&mut jb), Some(5.));
        let mut s = 6u32;
        for _ in 0..300 {
            for _ in 0..4 {
                push1(&mut jb, s, s as f32);
                s += 1;
            }
            for _ in 0..4 {
                assert!(pop(&mut jb).is_some());
            }
        }
        assert_eq!(jb.stats.dropped, 0);
        for _ in 0..5 {
            push1(&mut jb, s, s as f32);
            s += 1;
        }
        for _ in 0..450 {
            push1(&mut jb, s, s as f32);
            s += 1;
            pop(&mut jb);
        }
        assert_eq!((jb.stats.dropped, jb.depth()), (3, 2));
        for _ in 0..60 {
            push1(&mut jb, s, s as f32);
            s += 1;
        }
        assert_eq!(pop(&mut jb), Some((s - 3) as f32));
        push1(&mut jb, 10, 10.);
        assert_eq!(jb.stats.late, 1);
        for k in 0..3 {
            push1(&mut jb, 90000 + k, 100. + k as f32);
        }
        assert_eq!(pop(&mut jb), Some(100.));
    }

    #[test]
    fn multichannel_blocks_reassemble_and_tolerate_a_lost_group() {
        let mut jb = JitterBuffer::new(1);
        let n = 128;
        // block 0: 4 channels arriving as two packets (channels 0-1, then 2-3)
        let a: Vec<f32> = [vec![1.0; n], vec![2.0; n]].concat();
        let b: Vec<f32> = [vec![3.0; n], vec![4.0; n]].concat();
        jb.push(0, 4, 2, 2, n, &b);
        jb.push(0, 4, 0, 2, n, &a);
        let mut out = vec![0f32; MAX_CH * MAX_FRAME];
        assert_eq!(jb.pop(&mut out, n), Some(4));
        for ch in 0..4 {
            assert_eq!(out[ch * MAX_FRAME], (ch + 1) as f32);
        }
        // block 1: second group lost -> channels 2-3 silent, 0-1 still play
        jb.push(1, 4, 0, 2, n, &a);
        jb.push(2, 4, 0, 2, n, &a); // keeps the buffer moving
        assert_eq!(jb.pop(&mut out, n), Some(4));
        assert_eq!((out[0], out[MAX_FRAME], out[2 * MAX_FRAME], out[3 * MAX_FRAME]), (1.0, 2.0, 0.0, 0.0));
    }
}
