//! `sleep` without holding an OS thread: one timer thread wakes the sleepers.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::{Condvar, Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use crate::scheduler::{current_waiter, in_task, park};
use crate::task::Waiter;

struct Sleeper {
    deadline: Instant,
    seq: u64,
    waiter: Waiter,
}

impl PartialEq for Sleeper {
    fn eq(&self, other: &Self) -> bool {
        (self.deadline, self.seq) == (other.deadline, other.seq)
    }
}
impl Eq for Sleeper {}
impl PartialOrd for Sleeper {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Sleeper {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.deadline, self.seq).cmp(&(other.deadline, other.seq))
    }
}

static SLEEPERS: Mutex<(BinaryHeap<Reverse<Sleeper>>, u64)> = Mutex::new((BinaryHeap::new(), 0));
static CHANGED: Condvar = Condvar::new();
static TIMER: Once = Once::new();

fn timer() {
    let mut sleepers = SLEEPERS.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        let now = Instant::now();
        while sleepers.0.peek().is_some_and(|Reverse(s)| s.deadline <= now) {
            let Reverse(sleeper) = sleepers.0.pop().expect("peeked");
            sleeper.waiter.wake();
        }
        sleepers = match sleepers.0.peek() {
            Some(Reverse(next)) => {
                let wait = next.deadline.saturating_duration_since(now);
                CHANGED.wait_timeout(sleepers, wait).unwrap_or_else(PoisonError::into_inner).0
            }
            None => CHANGED.wait(sleepers).unwrap_or_else(PoisonError::into_inner),
        };
    }
}

/// Suspends the current task (or thread) for `duration`.
pub fn sleep(duration: Duration) {
    if !in_task() {
        std::thread::sleep(duration);
        return;
    }
    TIMER.call_once(|| {
        std::thread::Builder::new().name("grenat-timer".into()).spawn(timer).expect("timer thread");
    });
    let deadline = Instant::now() + duration;
    {
        let mut sleepers = SLEEPERS.lock().unwrap_or_else(PoisonError::into_inner);
        sleepers.1 += 1;
        let seq = sleepers.1;
        sleepers.0.push(Reverse(Sleeper { deadline, seq, waiter: current_waiter() }));
    }
    CHANGED.notify_one();
    while Instant::now() < deadline {
        park();
    }
}
