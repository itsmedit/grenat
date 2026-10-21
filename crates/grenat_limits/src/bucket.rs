//! A token bucket, kept as the instant it is next empty (the generic cell
//! rate algorithm): a call reserves its turn, and is told when it may go.
//! A turn booked but not taken (its caller cancelled) is given back only
//! while it is the last one booked: rewinding past turns booked after it
//! would hand one turn to two calls.

use std::time::{Duration, Instant};

use crate::Rate;

/// A call's booking: when it may go, and what it booked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub at: Instant,
    /// The bucket's `next` before, and after, this booking.
    before: Instant,
    after: Instant,
}

/// The calls a [`Rate`] allows: `count` at once from a full bucket, then
/// one each `period / count`, in the order they were reserved.
#[derive(Debug, Clone)]
pub struct Bucket {
    interval: Duration,
    /// How far ahead of now the bucket may be booked: `count - 1` intervals.
    burst: Duration,
    /// The theoretical arrival time of the next call: `None` while full.
    next: Option<Instant>,
}

impl Bucket {
    pub fn new(rate: Rate) -> Bucket {
        let interval = rate.interval();
        Bucket { interval, burst: interval * (rate.count - 1), next: None }
    }

    /// Books a call asked at `now`: its turn, at the instant it may go
    /// (`now` itself when the bucket holds a token).
    pub fn reserve(&mut self, now: Instant) -> Turn {
        let next = self.next.unwrap_or(now).max(now);
        let at = next.checked_sub(self.burst).map_or(now, |earliest| earliest.max(now));
        let after = next + self.interval;
        self.next = Some(after);
        Turn { at, before: next, after }
    }

    /// Gives back `turn`, booked but not taken (its caller was cancelled),
    /// if no turn was booked after it; otherwise it stays spent.
    pub fn refund(&mut self, turn: Turn) {
        if self.next == Some(turn.after) {
            self.next = Some(turn.before);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_full_bucket_then_one_call_per_interval() {
        let t0 = Instant::now();
        let mut bucket = Bucket::new(Rate::new(3, 3.0).unwrap());
        let times: Vec<Duration> = (0..6).map(|_| bucket.reserve(t0).at - t0).collect();
        assert_eq!(times, [ms(0), ms(0), ms(0), ms(1000), ms(2000), ms(3000)]);
    }

    #[test]
    fn a_quiet_while_fills_it_again() {
        let t0 = Instant::now();
        let mut bucket = Bucket::new(Rate::new(2, 1.0).unwrap());
        for _ in 0..4 {
            bucket.reserve(t0);
        }
        let later = t0 + Duration::from_secs(10);
        assert_eq!(bucket.reserve(later).at, later);
        assert_eq!(bucket.reserve(later).at, later);
        assert_eq!(bucket.reserve(later).at - later, ms(500));
    }

    #[test]
    fn calls_spread_over_time_never_wait() {
        let t0 = Instant::now();
        let mut bucket = Bucket::new(Rate::new(1, 1.0).unwrap());
        for i in 0..5 {
            let now = t0 + ms(1000 * i);
            assert_eq!(bucket.reserve(now).at, now);
        }
        assert_eq!(bucket.reserve(t0 + ms(4500)).at - t0, ms(5000));
    }

    #[test]
    fn a_refund_gives_the_last_turn_back() {
        let t0 = Instant::now();
        let mut bucket = Bucket::new(Rate::new(1, 1.0).unwrap());
        let first = bucket.reserve(t0);
        let second = bucket.reserve(t0);
        assert_eq!(second.at - t0, ms(1000));
        bucket.refund(second);
        let again = bucket.reserve(t0);
        assert_eq!(again.at - t0, ms(1000));
        // given back twice: once only
        bucket.refund(again);
        bucket.refund(again);
        assert_eq!(bucket.reserve(t0).at - t0, ms(1000));
        // an earlier turn, others booked after it: spent
        bucket.refund(first);
        assert_eq!(bucket.reserve(t0).at - t0, ms(2000));
    }

    #[test]
    fn a_turn_booked_before_others_stays_spent() {
        let t0 = Instant::now();
        let mut bucket = Bucket::new(Rate::new(1, 1.0).unwrap());
        bucket.reserve(t0);
        let b = bucket.reserve(t0);
        let c = bucket.reserve(t0);
        bucket.refund(b);
        let d = bucket.reserve(t0);
        assert_eq!((c.at - t0, d.at - t0), (ms(2000), ms(3000)));
    }
}
