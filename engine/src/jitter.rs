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
const AUTO_MAX: i64 = SLOTS as i64 / 2;
const QUIET: f32 = 0.01; // -40 dB: a block this quiet can be skipped without a click

/// Automatic target: raise on a dropout, lower after a calm stretch in which a
/// block always stood spare. Times are in blocks (set from the block rate).
struct Auto {
    on: bool,
    hold: u32,       // blocks between two raises, so one late burst counts once
    calm_base: u32,  // calm stretch needed before a step down
    calm_max: u32,
    calm_needed: u32,
    calm: u32,       // blocks since the last dropout or step
    since_raise: u32,
    min_depth: i64,  // lowest depth seen at playout during `calm`
    lowered: bool,   // the last step was down: a dropout right after means back off
    drop_wait: u32,  // how long a pending step down has waited for a quiet block
    drop_force: u32,
    pending_drop: bool,
    late: bool,      // a block arrived after its turn: more buffer would have saved it
    stall: bool,     // hold playout back one block, to make the raised target real
}

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
    auto: Auto,
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
            auto: Auto {
                on: false, hold: 20, calm_base: 3750, calm_max: 45000, calm_needed: 3750, calm: 0,
                since_raise: u32::MAX, min_depth: i64::MAX, lowered: false, drop_wait: 0, drop_force: 750,
                pending_drop: false, late: false, stall: false,
            },
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
        self.auto.calm = 0;
        self.auto.min_depth = i64::MAX;
        self.auto.pending_drop = false;
    }

    /// Manual target: switches automatic mode off.
    pub fn set_target(&mut self, target: i64) {
        self.target = target;
        self.auto.on = false;
        self.auto.pending_drop = false;
    }

    /// Automatic mode, timed for `blocks_per_sec` blocks a second: a raise at
    /// most every ~50 ms, a step down after 10 s without trouble (backing off to
    /// up to 2 minutes when steps down keep ending in a dropout).
    pub fn set_auto(&mut self, on: bool, blocks_per_sec: f64) {
        let a = &mut self.auto;
        let secs = |s: f64| (s * blocks_per_sec).round().max(1.0) as u32;
        a.on = on;
        a.hold = secs(0.05);
        a.calm_base = secs(10.0);
        a.calm_max = secs(120.0);
        a.calm_needed = a.calm_base;
        a.drop_force = secs(2.0);
        a.calm = 0;
        a.min_depth = i64::MAX;
        a.lowered = false;
        a.pending_drop = false;
        a.late = false;
        a.stall = false;
    }

    pub fn is_auto(&self) -> bool {
        self.auto.on
    }

    /// Bookkeeping for one playout in automatic mode; `depth` is what was
    /// buffered (including the block about to play), `dropout` whether it ran dry.
    fn auto_step(&mut self, depth: i64, dropout: bool) {
        let a = &mut self.auto;
        if !a.on {
            return;
        }
        a.since_raise = a.since_raise.saturating_add(1);
        if dropout || std::mem::take(&mut a.late) {
            if a.since_raise >= a.hold && self.target < AUTO_MAX {
                self.target += 1;
                a.since_raise = 0;
                a.stall = !dropout; // a dropout already held playout back by a block
                if a.lowered {
                    a.calm_needed = (a.calm_needed * 2).min(a.calm_max);
                }
                a.lowered = false;
            }
            a.pending_drop = false;
            a.calm = 0;
            a.min_depth = i64::MAX;
            return;
        }
        a.min_depth = a.min_depth.min(depth);
        a.calm += 1;
        if a.calm >= a.calm_needed {
            // a spare block the whole time: one less will do
            if a.min_depth >= 2 && self.target > 1 && !a.pending_drop {
                self.target -= 1;
                a.pending_drop = true;
                a.drop_wait = 0;
                a.lowered = true;
            } else if a.min_depth < 2 {
                a.lowered = false; // settled here: next time start from the short wait again
                a.calm_needed = a.calm_base;
            }
            a.calm = 0;
            a.min_depth = i64::MAX;
        }
    }

    /// The step down after a lower target: skip one block, preferably a quiet one.
    fn take_pending_drop(&mut self, next: i64) -> bool {
        if !self.auto.pending_drop {
            return false;
        }
        let slot = &self.slots[Self::index(next)];
        let behind = &self.slots[Self::index(next + 1)];
        if slot.seq != next || behind.seq != next + 1 {
            return false; // don't make a gap where nothing follows
        }
        self.auto.drop_wait += 1;
        let quiet = (0..slot.channels)
            .filter(|ch| slot.mask & (1 << ch) != 0)
            .all(|ch| slot.data[ch * MAX_FRAME..ch * MAX_FRAME + slot.len].iter().all(|v| v.abs() < QUIET));
        if !quiet && self.auto.drop_wait < self.auto.drop_force {
            return false;
        }
        self.slots[Self::index(next)].seq = -1;
        self.auto.pending_drop = false;
        self.stats.dropped += 1;
        true
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
                self.auto.late |= n - seq <= AUTO_MAX;
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
    /// channel). Returns (channel count, the block's sequence number), or None when
    /// there's nothing to play.
    pub fn pop(&mut self, out: &mut [f32], n: usize) -> Option<(usize, i64)> {
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
        self.auto_step((self.highest - next + 1).max(0), self.slots[Self::index(next)].seq != next && self.highest <= next);
        if std::mem::take(&mut self.auto.stall) {
            return None;
        }
        if self.take_pending_drop(next) {
            next += 1;
        }

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
            return Some((slot.channels, next));
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
        assert_eq!(jb.pop(&mut out, n), Some((4, 0)));
        for ch in 0..4 {
            assert_eq!(out[ch * MAX_FRAME], (ch + 1) as f32);
        }
        // block 1: second group lost -> channels 2-3 silent, 0-1 still play
        jb.push(1, 4, 0, 2, n, &a);
        jb.push(2, 4, 0, 2, n, &a); // keeps the buffer moving
        assert_eq!(jb.pop(&mut out, n), Some((4, 1)));
        assert_eq!((out[0], out[MAX_FRAME], out[2 * MAX_FRAME], out[3 * MAX_FRAME]), (1.0, 2.0, 0.0, 0.0));
    }

    /// Plays ticks `from..to`; block s arrives at tick s + delay(s). Returns what
    /// played (None for a dropout) per tick.
    fn run(jb: &mut JitterBuffer, from: u32, to: u32, delay: impl Fn(u32) -> u32, level: impl Fn(u32) -> f32) -> Vec<Option<f32>> {
        let mut inbox: Vec<(u32, u32)> = Vec::new(); // (arrival tick, seq)
        let mut played = Vec::new();
        for t in from..to {
            inbox.push((t + delay(t), t));
            inbox.retain(|&(at, s)| {
                if at <= t {
                    jb.push(s, 1, 0, 1, 128, &[level(s); 128]);
                }
                at > t
            });
            played.push(pop(jb));
        }
        played
    }

    #[test]
    fn auto_steps_down_on_a_clean_line() {
        let mut jb = JitterBuffer::new(3);
        jb.set_auto(true, 375.0); // 128 @ 48k
        let played = run(&mut jb, 0, 375 * 25, |_| 0, |_| 0.0);
        assert_eq!(jb.target, 1);
        assert_eq!(jb.stats.underruns, 0);
        assert_eq!(jb.stats.dropped, 2); // one block per step
        assert!(played.iter().skip(10).all(|p| p.is_some()));
    }

    #[test]
    fn auto_grows_to_cover_jitter_and_stays_there() {
        let mut jb = JitterBuffer::new(1);
        jb.set_auto(true, 375.0);
        // a wobbly line: most blocks on time, some up to 5 blocks (13 ms) late
        let delay = |s: u32| {
            let r = s.wrapping_mul(2654435761) >> 24;
            if r < 200 { 0 } else if r < 245 { 2 } else { 5 }
        };
        run(&mut jb, 0, 375 * 10, delay, |_| 0.0);
        assert!((4..=8).contains(&jb.target), "target {}", jb.target);
        let before = jb.stats.underruns + jb.stats.late;
        run(&mut jb, 375 * 10, 375 * 130, delay, |_| 0.0);
        // tries a step down now and then, backing off: only a handful of dropouts in two minutes
        let after = jb.stats.underruns + jb.stats.late;
        assert!(after - before <= 8, "{} dropouts / late blocks", after - before);
    }

    #[test]
    fn auto_step_down_waits_for_a_quiet_block() {
        let mut jb = JitterBuffer::new(2);
        jb.set_auto(true, 375.0);
        // loud except every 1000th block
        let level = |s: u32| if s % 1000 == 999 { 0.0 } else { 0.5 };
        let ticks = 375 * 10 + 1000;
        let played = run(&mut jb, 0, ticks, |_| 0, level);
        assert_eq!((jb.target, jb.stats.dropped), (1, 1));
        let quiet_sent = (0..ticks).filter(|&s| level(s) == 0.0).count();
        let quiet_played = played.iter().filter(|p| **p == Some(0.0)).count();
        assert_eq!(quiet_played, quiet_sent - 1, "the quiet block is the one skipped");
    }

    #[test]
    fn manual_target_turns_auto_off() {
        let mut jb = JitterBuffer::new(3);
        jb.set_auto(true, 375.0);
        jb.set_target(5);
        assert!(!jb.is_auto());
        run(&mut jb, 0, 375 * 25, |_| 0, |_| 0.0);
        assert_eq!(jb.target, 5);
    }
}
