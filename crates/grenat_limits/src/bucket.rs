//! A token bucket, kept as the instant it is next empty (the generic cell
//! rate algorithm): a call reserves its turn, and is told when it may go.

use std::time::{Duration, Instant};

use crate::Rate;

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

    /// Books a call asked at `now`: the instant it may go (`now` itself
    /// when the bucket holds a token).
    pub fn reserve(&mut self, now: Instant) -> Instant {
        let next = self.next.unwrap_or(now).max(now);
        let at = next.checked_sub(self.burst).map_or(now, |earliest| earliest.max(now));
        self.next = Some(next + self.interval);
        at
    }

    /// Gives back a call booked but not made (its caller was cancelled).
    pub fn refund(&mut self) {
        if let Some(next) = self.next {
            self.next = next.checked_sub(self.interval);
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
        let times: Vec<Duration> = (0..6).map(|_| bucket.reserve(t0) - t0).collect();
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
        assert_eq!(bucket.reserve(later), later);
        assert_eq!(bucket.reserve(later), later);
        assert_eq!(bucket.reserve(later) - later, ms(500));
    }

    #[test]
    fn calls_spread_over_time_never_wait() {
        let t0 = Instant::now();
        let mut bucket = Bucket::new(Rate::new(1, 1.0).unwrap());
        for i in 0..5 {
            let now = t0 + ms(1000 * i);
            assert_eq!(bucket.reserve(now), now);
        }
        assert_eq!(bucket.reserve(t0 + ms(4500)) - t0, ms(5000));
    }

    #[test]
    fn a_refund_gives_the_turn_back() {
        let t0 = Instant::now();
        let mut bucket = Bucket::new(Rate::new(1, 1.0).unwrap());
        bucket.reserve(t0);
        assert_eq!(bucket.reserve(t0) - t0, ms(1000));
        bucket.refund();
        assert_eq!(bucket.reserve(t0) - t0, ms(1000));
        bucket.refund();
        bucket.refund();
        assert_eq!(bucket.reserve(t0), t0);
    }
}
