//! What a collector's answer to an export means
//! (<https://opentelemetry.io/docs/specs/otlp/#otlphttp-response>):
//!
//! - a `2xx` is accepted, but for the spans a *partial success* names
//!   (`{"partialSuccess": {"rejectedSpans": "3", "errorMessage": "…"}}`):
//!   those are refused for good, and counted as dropped;
//! - `429`, `502`, `503` and `504` are worth sending again — after the
//!   `Retry-After` the collector gives (seconds or an HTTP date), if it does;
//! - a redirect is not followed (the headers carry API keys, a host the
//!   collector sends to must not get them): a failed export, as is any
//!   other answer.

use std::time::{Duration, SystemTime};

/// What became of one export.
#[derive(Debug, Clone, PartialEq)]
pub enum Export {
    /// Accepted; `rejected` of its spans refused by the collector, and why.
    Accepted { rejected: u64, message: Option<String> },
    /// Not accepted for now: to be sent again, after `retry_after` if the
    /// collector said.
    Retry { why: String, retry_after: Option<Duration> },
    /// Not accepted, never to be sent again.
    Failed(String),
}

/// The meaning of `status`, with the answer's `Retry-After` header and
/// its body (read for a `2xx` only), at `now`.
pub fn classify(status: u16, retry_after: Option<&str>, body: &[u8], now: SystemTime) -> Export {
    match status {
        200..=299 => partial_success(body),
        429 | 502 | 503 | 504 => Export::Retry {
            why: format!("collector answered {status}"),
            retry_after: retry_after.and_then(|value| grenat_limits::retry_after::parse(value, now)),
        },
        300..=399 => Export::Failed(format!("collector answered {status}: a redirect, never followed")),
        _ => Export::Failed(format!("collector answered {status}")),
    }
}

/// An accepted answer's `partialSuccess`, if it names rejected spans
/// (`rejectedSpans` is an int64: a string in OTLP JSON, a number read too).
fn partial_success(body: &[u8]) -> Export {
    let json: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let partial = &json["partialSuccess"];
    let rejected = match &partial["rejectedSpans"] {
        serde_json::Value::String(text) => text.parse().unwrap_or(0),
        number => number.as_u64().unwrap_or(0),
    };
    let message = partial["errorMessage"].as_str().filter(|m| !m.is_empty()).map(str::to_string);
    Export::Accepted { rejected, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(status: u16, retry_after: Option<&str>, body: &str) -> Export {
        classify(status, retry_after, body.as_bytes(), SystemTime::UNIX_EPOCH)
    }

    #[test]
    fn accepted_whole_or_in_part() {
        assert_eq!(at(200, None, "{}"), Export::Accepted { rejected: 0, message: None });
        assert_eq!(at(202, None, ""), Export::Accepted { rejected: 0, message: None });
        let partial = r#"{"partialSuccess": {"rejectedSpans": "3", "errorMessage": "too old"}}"#;
        assert_eq!(at(200, None, partial), Export::Accepted { rejected: 3, message: Some("too old".into()) });
        let number = r#"{"partialSuccess": {"rejectedSpans": 2}}"#;
        assert_eq!(at(200, None, number), Export::Accepted { rejected: 2, message: None });
        // an empty partial success is a full one
        assert_eq!(at(200, None, r#"{"partialSuccess": {}}"#), Export::Accepted { rejected: 0, message: None });
    }

    #[test]
    fn worth_sending_again_after_what_the_collector_says() {
        for status in [429, 502, 503, 504] {
            assert!(matches!(at(status, None, ""), Export::Retry { retry_after: None, .. }), "{status}");
        }
        assert_eq!(
            at(503, Some("7"), ""),
            Export::Retry { why: "collector answered 503".into(), retry_after: Some(Duration::from_secs(7)) }
        );
        let date = at(429, Some("Thu, 01 Jan 1970 00:00:30 GMT"), "");
        assert!(matches!(date, Export::Retry { retry_after: Some(d), .. } if d == Duration::from_secs(30)));
    }

    #[test]
    fn never_sent_again() {
        for status in [400, 401, 403, 404, 413, 500, 501] {
            assert!(matches!(at(status, Some("1"), ""), Export::Failed(_)), "{status}");
        }
        let Export::Failed(why) = at(302, None, "") else { panic!("a redirect") };
        assert!(why.contains("redirect"), "{why}");
    }
}
