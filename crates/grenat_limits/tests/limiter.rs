//! The limiter as tasks see it: a clock moved by hand for the waits a rate
//! imposes, real threads sharing one limiter, cancellation, pauses.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use grenat_limits::{Limiter, Rate};

/// A clock that moves only when a call waits.
struct Clock {
    start: Instant,
    now: Cell<Instant>,
}

impl Clock {
    fn new() -> Clock {
        let start = Instant::now();
        Clock { start, now: Cell::new(start) }
    }

    fn now(&self) -> Instant {
        self.now.get()
    }

    fn wait(&self, d: Duration) -> Result<(), ()> {
        self.now.set(self.now.get() + d);
        Ok(())
    }

    fn elapsed(&self) -> Duration {
        self.now.get() - self.start
    }
}

fn limiter(count: i64, seconds: f64, concurrent: Option<usize>) -> Arc<Limiter> {
    Arc::new(Limiter::new(Some(Rate::new(count, seconds).unwrap()), concurrent))
}

#[test]
fn calls_over_the_rate_wait_their_turn() {
    let clock = Clock::new();
    let limiter = limiter(10, 1.0, None);
    let mut waits = Vec::new();
    for _ in 0..25 {
        let permit = limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
        waits.push(permit.waited);
    }
    // ten at once, then one each 100 ms
    assert!(waits[..10].iter().all(|w| w.is_zero()));
    assert!(waits[10..].iter().all(|w| *w == Duration::from_millis(100)));
    assert_eq!(clock.elapsed(), Duration::from_millis(1500));
}

#[test]
fn a_cancelled_wait_gives_its_turn_back() {
    let clock = Clock::new();
    let limiter = limiter(1, 1.0, None);
    limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
    // the next call is cancelled while it waits
    assert_eq!(limiter.acquire(|| clock.now(), |_| Err("cancelled")).unwrap_err(), "cancelled");
    // its turn was given back: the call after it waits one interval, not two
    let permit = limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
    assert_eq!(permit.waited, Duration::from_secs(1));
}

#[test]
fn a_cancelled_turn_booked_before_others_is_never_given_to_two_calls() {
    let clock = Clock::new();
    let limiter = limiter(1, 1.0, None);
    limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
    // B waits for its turn (1 s); meanwhile C books the next one (2 s) and
    // goes; then B is cancelled
    let mut c_waited = None;
    let cancelled = limiter.acquire(
        || clock.now(),
        |_| {
            c_waited = Some(limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap().waited);
            Err("cancelled")
        },
    );
    assert_eq!(cancelled.unwrap_err(), "cancelled");
    assert_eq!(c_waited, Some(Duration::from_secs(2)));
    // B's turn is not the last booked: D still waits for the turn after C's
    let d = limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
    assert_eq!(d.waited, Duration::from_secs(1), "C and D would share one turn");
}

#[test]
fn a_call_cancelled_during_a_pause_gives_its_turn_back() {
    let clock = Clock::new();
    let limiter = limiter(1, 1.0, None);
    limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
    let start = clock.now();
    let mut waits = 0;
    let cancelled = limiter.acquire(
        || clock.now(),
        |d| {
            waits += 1;
            if waits == 1 {
                // its turn comes, and with it a pause of half a second
                clock.wait(d)?;
                limiter.pause_until(start + Duration::from_millis(1500));
                return Ok(());
            }
            clock.wait(Duration::from_millis(200))?;
            Err(())
        },
    );
    assert!(cancelled.is_err());
    // the turn given back: the next call goes when the pause ends
    let d = limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
    assert_eq!(d.waited, Duration::from_millis(300));
}

#[test]
fn a_pause_holds_every_call_back() {
    let clock = Clock::new();
    let limiter = limiter(100, 1.0, None);
    limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
    limiter.pause_until(clock.now() + Duration::from_secs(30));
    // an earlier pause does not shorten it
    limiter.pause_until(clock.now() + Duration::from_secs(5));
    let permit = limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
    assert_eq!(permit.waited, Duration::from_secs(30));
    // once over, calls go at once
    assert!(limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap().waited.is_zero());
}

#[test]
fn a_refill_forgets_turns_and_pauses() {
    let clock = Clock::new();
    let limiter = limiter(1, 60.0, None);
    limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap();
    limiter.pause_until(clock.now() + Duration::from_secs(300));
    limiter.refill();
    assert!(limiter.acquire(|| clock.now(), |d| clock.wait(d)).unwrap().waited.is_zero());
}

#[test]
fn concurrency_alone_never_waits_for_a_free_slot() {
    let limiter = Arc::new(Limiter::new(None, Some(2)));
    let a = limiter.acquire(Instant::now, |_| Err(())).unwrap();
    let b = limiter.acquire(Instant::now, |_| Err(())).unwrap();
    assert_eq!(limiter.in_flight(), 2);
    // full: a third waits, and is cancelled
    assert!(limiter.acquire(Instant::now, |_| Err(())).is_err());
    drop(a);
    assert_eq!(limiter.in_flight(), 1);
    let c = limiter.acquire(Instant::now, |_| Err(())).unwrap();
    drop((b, c));
    assert_eq!(limiter.in_flight(), 0);
}

#[test]
fn threads_never_exceed_the_concurrency_limit() {
    let limiter = Arc::new(Limiter::new(None, Some(3)));
    let (current, highest) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let threads: Vec<_> = (0..12)
        .map(|_| {
            let (limiter, current, highest) = (limiter.clone(), current.clone(), highest.clone());
            std::thread::spawn(move || {
                let _permit = limiter
                    .acquire(Instant::now, |d| {
                        std::thread::sleep(d);
                        Ok::<(), ()>(())
                    })
                    .unwrap();
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                highest.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
                current.fetch_sub(1, Ordering::SeqCst);
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(highest.load(Ordering::SeqCst), 3);
    assert_eq!(limiter.in_flight(), 0);
}

#[test]
fn threads_share_one_bucket() {
    // 5 at once, then one each 50 ms: 15 calls take 500 ms at least
    let limiter = limiter(5, 0.25, None);
    let started = Instant::now();
    let times = Arc::new(Mutex::new(Vec::new()));
    let threads: Vec<_> = (0..15)
        .map(|_| {
            let (limiter, times) = (limiter.clone(), times.clone());
            std::thread::spawn(move || {
                limiter
                    .acquire(Instant::now, |d| {
                        std::thread::sleep(d);
                        Ok::<(), ()>(())
                    })
                    .unwrap();
                times.lock().unwrap().push(started.elapsed());
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let mut times = times.lock().unwrap().clone();
    times.sort();
    assert!(times[14] >= Duration::from_millis(500), "{times:?}");
    // never more than the rate allows: the k-th call (from 0) after a full
    // bucket of 5 goes no sooner than (k - 4) intervals
    for (k, t) in times.iter().enumerate().skip(5) {
        assert!(*t >= Duration::from_millis(50 * (k as u64 - 4)), "call {k} at {t:?}");
    }
}

#[test]
fn slots_then_tokens() {
    // one in flight, one each 200 ms: the second call waits for the slot
    // (100 ms), then for its token (200 ms after the first one's)
    let limiter = limiter(1, 0.2, Some(1));
    let sleep = |d: Duration| {
        std::thread::sleep(d);
        Ok::<(), ()>(())
    };
    let first = limiter.acquire(Instant::now, sleep).unwrap();
    let started = Instant::now();
    let holder = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(first);
    });
    let second = limiter.acquire(Instant::now, sleep).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(150), "{:?}", started.elapsed());
    assert!(second.waited >= Duration::from_millis(150));
    assert_eq!(limiter.in_flight(), 1);
    holder.join().unwrap();
}
