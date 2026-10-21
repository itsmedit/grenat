//! When an export worth sending again is sent again
//! (<https://opentelemetry.io/docs/specs/otlp/#retryable-response-codes>):
//! after the `Retry-After` the collector gave, else after an exponential
//! backoff with jitter — 250 ms, 500 ms, 1 s… capped at 5 s, each between
//! half and all of it, so that processes started together do not retry
//! together. [`MAX_ATTEMPTS`] in all; a `Retry-After` longer than
//! [`MAX_RETRY_AFTER`] gives up at once (the batch would wait longer than
//! a run may last). All of it on the exporter's thread: the program never
//! waits.

use std::time::Duration;

/// Sends of one batch, the first included.
pub const MAX_ATTEMPTS: u32 = 4;

/// The longest `Retry-After` waited out.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

const FIRST_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// How long to wait after `attempts` sends (one at least) before the next,
/// the collector having asked `retry_after`; `jitter` in `[0, 1)`. `None`:
/// no more attempts.
pub fn delay(attempts: u32, retry_after: Option<Duration>, jitter: f64) -> Option<Duration> {
    if attempts >= MAX_ATTEMPTS {
        return None;
    }
    if let Some(asked) = retry_after {
        return (asked <= MAX_RETRY_AFTER).then_some(asked);
    }
    let backoff = FIRST_BACKOFF.saturating_mul(1 << (attempts.max(1) - 1).min(16)).min(MAX_BACKOFF);
    Some(backoff.mul_f64(0.5 + jitter.clamp(0.0, 1.0) / 2.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn an_exponential_backoff_with_jitter_bounded_in_attempts() {
        assert_eq!(delay(1, None, 0.999_999).map(|d| d.as_millis()), Some(249));
        assert_eq!(delay(1, None, 0.0), Some(ms(125)));
        assert_eq!(delay(2, None, 0.0), Some(ms(250)));
        assert_eq!(delay(3, None, 0.0), Some(ms(500)));
        assert_eq!(delay(MAX_ATTEMPTS, None, 0.0), None);
    }

    #[test]
    fn the_backoff_is_capped() {
        // were there more attempts, never more than 5 s
        let capped = FIRST_BACKOFF.saturating_mul(1 << 16).min(MAX_BACKOFF);
        assert_eq!(capped, MAX_BACKOFF);
    }

    #[test]
    fn what_the_collector_asks_is_waited_unless_too_long() {
        assert_eq!(delay(1, Some(ms(1500)), 0.3), Some(ms(1500)));
        assert_eq!(delay(1, Some(Duration::ZERO), 0.3), Some(Duration::ZERO));
        assert_eq!(delay(1, Some(Duration::from_secs(31)), 0.3), None);
        assert_eq!(delay(MAX_ATTEMPTS, Some(ms(1)), 0.3), None);
    }
}
