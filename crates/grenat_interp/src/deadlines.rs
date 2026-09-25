//! Deadlines: a flag raised when a duration has passed, by one watcher
//! thread for all of them (a tool that runs too long is cancelled at its
//! next checkpoint).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

struct Watcher {
    pending: Mutex<Vec<(Instant, Arc<AtomicBool>)>>,
    changed: Condvar,
}

fn watcher() -> &'static Watcher {
    static WATCHER: OnceLock<&'static Watcher> = OnceLock::new();
    WATCHER.get_or_init(|| {
        let watcher: &'static Watcher =
            Box::leak(Box::new(Watcher { pending: Mutex::new(Vec::new()), changed: Condvar::new() }));
        std::thread::Builder::new()
            .name("grenat-deadlines".into())
            .spawn(move || watch(watcher))
            .expect("deadline watcher thread");
        watcher
    })
}

fn watch(watcher: &Watcher) {
    let mut pending = watcher.pending.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        let now = Instant::now();
        pending.retain(|(at, flag)| {
            let due = *at <= now;
            if due {
                flag.store(true, Ordering::Relaxed);
            }
            !due && Arc::strong_count(flag) > 1
        });
        let next = pending.iter().map(|(at, _)| *at).min();
        pending = match next {
            Some(at) => watcher.changed.wait_timeout(pending, at - now).unwrap_or_else(PoisonError::into_inner).0,
            None => watcher.changed.wait(pending).unwrap_or_else(PoisonError::into_inner),
        };
    }
}

/// A flag raised once `duration` has passed.
pub(crate) fn after(duration: Duration) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let watcher = watcher();
    watcher.pending.lock().unwrap_or_else(PoisonError::into_inner).push((Instant::now() + duration, flag.clone()));
    watcher.changed.notify_one();
    flag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_raised_when_due() {
        let (soon, later) = (after(Duration::from_millis(20)), after(Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(80));
        assert!(soon.load(Ordering::Relaxed));
        assert!(!later.load(Ordering::Relaxed));
    }
}
