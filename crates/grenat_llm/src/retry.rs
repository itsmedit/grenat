//! Calls that retry: rate limits, overload and server errors are retried
//! with a growing delay (or the one `retry-after` gives); client errors are
//! not.

use std::time::Duration;

use serde_json::Value as Json;

use crate::*;

pub(crate) const MAX_ATTEMPTS: u32 = 4;

/// What one attempt got: (status, `retry-after` seconds, body).
pub(crate) type Attempt = Result<(u16, Option<u64>, Json), String>;

/// Sends until a 200 (parsed by `parse`), a client error, or the last attempt.
pub(crate) fn with_retries<T>(
    delay: Duration,
    send: impl Fn() -> Attempt,
    parse: impl Fn(&Json) -> Result<T, LlmError>,
) -> Result<T, LlmError> {
    let mut last_error = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        let wait = match send() {
            Ok((200, _, json)) => return parse(&json),
            Ok((status, retry_after, json)) => {
                last_error = format!("HTTP {status}: {}", error_message(&json));
                if !retryable(status) {
                    break;
                }
                retry_after.map_or(delay * (1 << attempt), Duration::from_secs)
            }
            Err(e) => {
                last_error = e;
                delay * (1 << attempt)
            }
        };
        if attempt + 1 < MAX_ATTEMPTS {
            std::thread::sleep(wait.min(Duration::from_secs(30)));
        }
    }
    Err(LlmError::new(last_error))
}

/// Sends a stream until it is read whole, a failure not to retry, or the
/// last attempt.
pub(crate) fn with_stream_retries<T>(
    delay: Duration,
    mut attempt: impl FnMut() -> Result<T, crate::streaming::Failed>,
) -> Result<T, LlmError> {
    let mut last_error = LlmError::new("");
    for i in 0..MAX_ATTEMPTS {
        let failed = match attempt() {
            Ok(value) => return Ok(value),
            Err(failed) => failed,
        };
        last_error = LlmError { message: failed.message, billed: failed.billed };
        if !failed.retry {
            break;
        }
        if i + 1 < MAX_ATTEMPTS {
            let wait = failed.retry_after.map_or(delay * (1 << i), Duration::from_secs);
            std::thread::sleep(wait.min(Duration::from_secs(30)));
        }
    }
    Err(last_error)
}

/// 408, 409, 429, 5xx (including 529 "overloaded"): worth another attempt.
pub(crate) fn retryable(status: u16) -> bool {
    matches!(status, 408 | 409 | 429) || status >= 500
}

/// The message of an error body: `{"error": {"message": …}}` (also inside a
/// list, as some providers send it), or `{"detail": …}` (Voyage's).
pub(crate) fn error_message(json: &Json) -> &str {
    // not JSON: the text itself
    if let Some(text) = json.as_str() {
        return text.trim();
    }
    let error = if json.is_array() { &json[0]["error"] } else { &json["error"] };
    error["message"].as_str().or_else(|| error.as_str()).or_else(|| json["detail"].as_str()).unwrap_or("unknown error")
}
