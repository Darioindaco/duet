//! A small pool of worker threads for the mixer: the tracks of one stage of the mix (everything
//! that doesn't feed anything else in the same stage) are worked on at the same time, one per core.
//!
//! Made for the audio thread: running a job allocates nothing and takes no lock. The caller works
//! on the job too, and waits (spinning, it's short) until every item is done; workers spin a little
//! after a job and then sleep until the next. If the pool is busy (an export running alongside the
//! audio) or switched off, the caller simply does every item itself: the result is the same, only
//! slower.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Arc, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

/// Use more than one core (the setting; off: everything on the audio thread, as before).
pub static ENABLED: AtomicBool = AtomicBool::new(true);

type Job = *const (dyn Fn(usize) + Sync);

struct Shared {
    gen: AtomicU64,
    job: UnsafeCell<Option<Job>>,
    count: AtomicUsize,
    next: AtomicUsize,
    done: AtomicUsize,
    /// workers inside a job (the caller waits for 0 before the job's data can go)
    active: AtomicUsize,
    busy: AtomicBool,
    quit: AtomicBool,
}

// (job is only written by the one caller holding `busy`, while no worker is active)
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

pub struct Pool {
    shared: Arc<Shared>,
    threads: Vec<Thread>,
}

static POOL: OnceLock<Pool> = OnceLock::new();

/// The pool (made the first time: call `init` early, off the audio thread).
pub fn get() -> &'static Pool {
    POOL.get_or_init(|| {
        let cores = thread::available_parallelism().map_or(1, |n| n.get());
        Pool::new(cores.saturating_sub(1).min(7))
    })
}

pub fn init() {
    let _ = get();
}

impl Pool {
    fn new(workers: usize) -> Pool {
        let shared = Arc::new(Shared {
            gen: AtomicU64::new(0),
            job: UnsafeCell::new(None),
            count: AtomicUsize::new(0),
            next: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            busy: AtomicBool::new(false),
            quit: AtomicBool::new(false),
        });
        let mut threads = Vec::new();
        for k in 0..workers {
            let sh = shared.clone();
            if let Ok(h) = thread::Builder::new().name(format!("duet-mix-{k}")).spawn(move || worker(sh)) {
                threads.push(h.thread().clone());
            }
        }
        Pool { shared, threads }
    }

    pub fn workers(&self) -> usize {
        self.threads.len()
    }

    /// f(0), f(1) ... f(count - 1), spread over the cores; back when all are done.
    pub fn run(&self, count: usize, f: &(dyn Fn(usize) + Sync)) {
        let sh = &*self.shared;
        if count == 0 {
            return;
        }
        if count == 1 || self.threads.is_empty() || !ENABLED.load(Relaxed) || sh.busy.swap(true, Acquire) {
            for i in 0..count {
                f(i);
            }
            return;
        }
        // a worker still on its way out of the last job reads the next one's count: let it go first
        while sh.active.load(Acquire) > 0 {
            std::hint::spin_loop();
        }
        // (the lifetime is ours to keep: we don't return before every worker has let go of it)
        let job: Job = unsafe { std::mem::transmute::<*const (dyn Fn(usize) + Sync + '_), Job>(f as *const _) };
        unsafe { *sh.job.get() = Some(job) };
        sh.count.store(count, Relaxed);
        sh.done.store(0, Relaxed);
        sh.next.store(0, Relaxed);
        sh.gen.fetch_add(1, Release);
        for t in self.threads.iter().take(count - 1) {
            t.unpark();
        }
        // the caller works too
        loop {
            let i = sh.next.fetch_add(1, AcqRel);
            if i >= count {
                break;
            }
            f(i);
            sh.done.fetch_add(1, AcqRel);
        }
        while sh.done.load(Acquire) < count || sh.active.load(Acquire) > 0 {
            std::hint::spin_loop();
        }
        unsafe { *sh.job.get() = None };
        sh.busy.store(false, Release);
    }
}

fn worker(sh: Arc<Shared>) {
    let mut seen = 0u64;
    loop {
        // wait for a new job: spin a moment (the next block comes soon), then sleep
        let t0 = Instant::now();
        loop {
            if sh.quit.load(Relaxed) {
                return;
            }
            if sh.gen.load(Acquire) != seen {
                break;
            }
            if t0.elapsed() < Duration::from_micros(300) {
                std::hint::spin_loop();
            } else {
                thread::park_timeout(Duration::from_millis(50));
            }
        }
        sh.active.fetch_add(1, AcqRel);
        let g = sh.gen.load(Acquire);
        seen = g;
        let count = sh.count.load(Acquire);
        loop {
            let i = sh.next.fetch_add(1, AcqRel);
            if i >= count {
                break;
            }
            // (i < count: the caller is waiting for this item, so the job is still there)
            if let Some(job) = unsafe { *sh.job.get() } {
                unsafe { (*job)(i) };
            }
            sh.done.fetch_add(1, AcqRel);
        }
        sh.active.fetch_sub(1, AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn every_item_once_on_any_number_of_cores() {
        let p = Pool::new(3);
        for round in 0..2000 {
            let n = 1 + round % 13;
            let hits: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(0)).collect();
            p.run(n, &|i| {
                hits[i].fetch_add(1, Relaxed);
            });
            assert!(hits.iter().all(|h| h.load(Relaxed) == 1), "round {round}");
        }
    }

    #[test]
    fn busy_or_off_runs_on_the_caller() {
        let p = Pool::new(2);
        ENABLED.store(false, Relaxed);
        let hits = AtomicU32::new(0);
        p.run(5, &|_| {
            hits.fetch_add(1, Relaxed);
        });
        ENABLED.store(true, Relaxed);
        assert_eq!(hits.load(Relaxed), 5);
    }
}
