//! Jitter buffer: reorders incoming frames and plays them out at a fixed depth.
//! Same algorithm as the Python version (see engine.py), but with preallocated
//! slots so the audio thread never allocates.

pub const MAX_FRAME: usize = 1024;
const SLOTS: usize = 128;
const TRIM_WINDOW: u32 = 200; // blocks to watch before trimming drift
const HARD_TRIM: i64 = 32; // frames over target that get trimmed right away
const RESTART_GAP: i64 = 5000;
const STARVE_RESET: u32 = 400;

#[derive(Clone)]
struct Slot {
    seq: i64,
    len: usize,
    data: [f32; MAX_FRAME],
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
        let mut jb = JitterBuffer {
            target,
            slots: vec![Slot { seq: -1, len: 0, data: [0.0; MAX_FRAME] }; SLOTS],
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

    fn slot_mut(&mut self, seq: i64) -> &mut Slot {
        &mut self.slots[(seq as usize) % SLOTS]
    }

    fn take(&mut self, seq: i64) -> Option<usize> {
        let s = self.slot_mut(seq);
        if s.seq == seq {
            s.seq = -1;
            Some((seq as usize) % SLOTS)
        } else {
            None
        }
    }

    fn min_seq(&self) -> Option<i64> {
        self.slots.iter().filter(|s| s.seq >= 0).map(|s| s.seq).min()
    }

    pub fn push(&mut self, seq: u32, samples: &[f32]) {
        let seq = seq as i64;
        if let Some(n) = self.next {
            if (seq - n).abs() > RESTART_GAP {
                // partner restarted their audio: start over
                self.reset();
            }
        }
        if let Some(n) = self.next {
            if seq < n {
                self.stats.late += 1;
                return;
            }
        }
        let len = samples.len().min(MAX_FRAME);
        let slot = self.slot_mut(seq);
        slot.seq = seq;
        slot.len = len;
        slot.data[..len].copy_from_slice(&samples[..len]);
        if seq > self.highest {
            self.highest = seq;
        }
    }

    /// Fill `out` with the next frame. Returns false when there's nothing to play.
    pub fn pop(&mut self, out: &mut [f32]) -> bool {
        if !self.started {
            let Some(first) = self.min_seq() else { return false };
            if self.highest - first + 1 < self.target {
                return false;
            }
            self.next = Some(first.max(self.highest - self.target + 1));
            self.started = true;
            self.starved = 0;
        }
        let mut next = self.next.unwrap();

        // Too much queued: drivers deliver in bursts, so only trim when even the
        // lowest depth over ~half a second sits above target, or after a pile-up.
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
                self.take(s);
            }
            next += excess;
            self.stats.dropped += excess as u64;
            self.win_min = i64::MAX;
            self.win_count = 0;
        }

        if let Some(i) = self.take(next) {
            let slot = &self.slots[i];
            let n = slot.len.min(out.len());
            out[..n].copy_from_slice(&slot.data[..n]);
            out[n..].fill(0.0);
            self.next = Some(next + 1);
            self.starved = 0;
            return n == out.len();
        }
        if self.highest > next {
            // later frames are here, so this one is lost: move on
            self.stats.lost += 1;
            self.next = Some(next + 1);
            return false;
        }
        // Ran dry. Don't advance, so a merely late frame still plays (this also
        // re-adds a frame of delay, which corrects for a slow partner clock).
        self.next = Some(next);
        self.stats.underruns += 1;
        self.starved += 1;
        if self.starved > STARVE_RESET {
            self.reset();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(v: f32) -> Vec<f32> {
        vec![v; 128]
    }
    fn pop(jb: &mut JitterBuffer) -> Option<f32> {
        let mut out = [0f32; 128];
        jb.pop(&mut out).then_some(out[0])
    }

    // mirrors selftest.py's test_jitter_buffer step by step
    #[test]
    fn behaves_like_python_version() {
        let mut jb = JitterBuffer::new(3);
        assert_eq!(pop(&mut jb), None);
        for s in [1, 0, 2] {
            jb.push(s, &f(s as f32));
        }
        assert_eq!([pop(&mut jb), pop(&mut jb), pop(&mut jb)], [Some(0.), Some(1.), Some(2.)]);
        assert_eq!(pop(&mut jb), None);
        assert_eq!(jb.stats.underruns, 1);
        jb.push(3, &f(3.));
        assert_eq!(pop(&mut jb), Some(3.));
        jb.push(5, &f(5.));
        assert_eq!(pop(&mut jb), None);
        assert_eq!(jb.stats.lost, 1);
        assert_eq!(pop(&mut jb), Some(5.));
        // driver-style bursts never trim
        let mut s = 6u32;
        for _ in 0..300 {
            for _ in 0..4 {
                jb.push(s, &f(s as f32));
                s += 1;
            }
            for _ in 0..4 {
                assert!(pop(&mut jb).is_some());
            }
        }
        assert_eq!(jb.stats.dropped, 0);
        // sustained excess trimmed after the window
        for _ in 0..5 {
            jb.push(s, &f(s as f32));
            s += 1;
        }
        for _ in 0..450 {
            jb.push(s, &f(s as f32));
            s += 1;
            pop(&mut jb);
        }
        assert_eq!((jb.stats.dropped, jb.depth()), (3, 2));
        // huge pile-up trimmed at once
        for _ in 0..60 {
            jb.push(s, &f(s as f32));
            s += 1;
        }
        assert_eq!(pop(&mut jb), Some((s - 3) as f32));
        jb.push(10, &f(10.));
        assert_eq!(jb.stats.late, 1);
        for k in 0..3 {
            jb.push(90000 + k, &f(100. + k as f32));
        }
        assert_eq!(pop(&mut jb), Some(100.));
    }
}
