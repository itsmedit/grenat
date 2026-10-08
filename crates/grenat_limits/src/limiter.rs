//! A limiter: a rate, a concurrency limit, or both, and a pause; shared by
//! every task (behind an `Arc`), waited on through the caller's own sleep.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::{Bucket, Rate, Slots};

/// How long a call queued for a slot waits before looking again.
pub const SLOT_POLL: Duration = Duration::from_millis(10);

/// What `limit net("host"), rate, concurrent: n` declares.
#[derive(Debug)]
pub struct Limiter {
    rate: Option<Rate>,
    concurrent: Option<usize>,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    bucket: Option<Bucket>,
    slots: Option<Slots>,
    /// No call goes before then (a server's `Retry-After`).
    paused: Option<Instant>,
}

impl Limiter {
    pub fn new(rate: Option<Rate>, concurrent: Option<usize>) -> Limiter {
        Limiter { rate, concurrent, state: Mutex::new(State::fresh(rate, concurrent)) }
    }

    pub fn rate(&self) -> Option<Rate> {
        self.rate
    }

    pub fn concurrent(&self) -> Option<usize> {
        self.concurrent
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for a slot, then for a token, then for the end of a pause:
    /// the permit to call (its slot given back when dropped). `wait(d)` is
    /// the caller's sleep: it may sleep less than `d` (the limiter looks
    /// again), and an error from it — a cancelled task — gives back what
    /// was taken and is returned.
    pub fn acquire<E>(
        self: &Arc<Self>,
        now: impl Fn() -> Instant,
        mut wait: impl FnMut(Duration) -> Result<(), E>,
    ) -> Result<Permit, E> {
        let started = now();
        let mut permit = Permit { limiter: None, waited: Duration::ZERO };
        let ticket = self.state().slots.as_mut().map(Slots::enqueue);
        if let Some(ticket) = ticket {
            loop {
                if self.state().slots.as_mut().is_some_and(|s| s.try_enter(ticket)) {
                    permit.limiter = Some(self.clone());
                    break;
                }
                if let Err(e) = wait(SLOT_POLL) {
                    if let Some(slots) = self.state().slots.as_mut() {
                        slots.leave(ticket);
                    }
                    return Err(e);
                }
            }
        }
        let turn = {
            let mut state = self.state();
            let at = now();
            let from = state.paused.map_or(at, |p| p.max(at));
            state.bucket.as_mut().map(|b| b.reserve(from))
        };
        if let Some(turn) = turn {
            loop {
                let now = now();
                if now >= turn {
                    break;
                }
                if let Err(e) = wait(turn - now) {
                    if let Some(bucket) = self.state().bucket.as_mut() {
                        bucket.refund();
                    }
                    return Err(e);
                }
            }
        }
        // a pause that began while this call waited
        while let Some(left) = self.paused_for(now()) {
            wait(left)?;
        }
        permit.waited = now().saturating_duration_since(started);
        Ok(permit)
    }

    /// What is left of a pause at `now`, if any.
    fn paused_for(&self, now: Instant) -> Option<Duration> {
        self.state().paused.and_then(|p| p.checked_duration_since(now)).filter(|left| !left.is_zero())
    }

    /// No call before `until` (a later pause wins over an earlier one).
    pub fn pause_until(&self, until: Instant) {
        let mut state = self.state();
        state.paused = Some(state.paused.map_or(until, |p| p.max(until)));
    }

    /// A full bucket and no pause (between two tests); calls in flight
    /// keep their slots.
    pub fn refill(&self) {
        let mut state = self.state();
        state.bucket = self.rate.map(Bucket::new);
        state.paused = None;
    }

    /// The calls in flight.
    pub fn in_flight(&self) -> usize {
        self.state().slots.as_ref().map_or(0, Slots::in_flight)
    }

    fn release(&self) {
        if let Some(slots) = self.state().slots.as_mut() {
            slots.release();
        }
    }
}

impl State {
    fn fresh(rate: Option<Rate>, concurrent: Option<usize>) -> State {
        State { bucket: rate.map(Bucket::new), slots: concurrent.map(Slots::new), paused: None }
    }
}

/// The right to make one call; its slot, if any, is given back on drop.
#[derive(Debug)]
pub struct Permit {
    limiter: Option<Arc<Limiter>>,
    /// How long the call waited for it.
    pub waited: Duration,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(limiter) = self.limiter.take() {
            limiter.release();
        }
    }
}
