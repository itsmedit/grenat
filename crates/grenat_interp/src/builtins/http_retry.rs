//! A request answered `429 Too Many Requests` (RFC 6585) with a
//! `Retry-After` (RFC 9110, §10.2.3: seconds or an HTTP date) is sent again
//! once that time has passed: three times at most, and only when the server
//! asks for a minute or less — a longer wait, or a `429` without
//! `Retry-After`, is the program's answer to handle (`res.status`).
//!
//! Only a request that can be sent twice is: GET, HEAD, PUT and DELETE
//! are idempotent (RFC 9110, §9.2.2); POST and PATCH are not, and are sent
//! again only with `retry: true`. `retry: false` sends any request once.
//!
//! ```ruby
//! Http.get("https://api.github.com/repos/acme/app")                  # retried
//! Http.post("https://api.hubspot.com/crm/v3/objects/contacts", json: c, retry: true)
//! Http.put(url, json: doc, retry: false)                              # once
//! ```

use std::time::{Duration, SystemTime};

use crate::http::HttpReply;
use crate::prelude::*;

/// Attempts after the first.
pub(crate) const MAX_RETRIES: u32 = 3;

/// The longest `Retry-After` waited out.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(60);

/// Whether a request may be sent again: `retry:` as given, else whether its
/// method is idempotent.
pub(crate) fn allowed<'p>(method: &str, args: &Args<'p>) -> Result<bool, Ctrl<'p>> {
    match args.named.iter().find(|(n, _)| n == "retry").map(|(_, v)| v.untainted()) {
        Some(Value::Bool(given)) => Ok(*given),
        Some(other) => {
            raise("ArgumentError", format!("invalid `Http` option `retry: {}` (true or false)", other.inspect()))
        }
        None => Ok(matches!(method, "GET" | "HEAD" | "PUT" | "DELETE")),
    }
}

/// How long a `429` asks to wait, if it says.
pub(crate) fn asked_wait(reply: &HttpReply) -> Option<Duration> {
    if reply.status != 429 {
        return None;
    }
    let (_, value) = reply.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))?;
    grenat_limits::retry_after::parse(value, SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16, retry_after: Option<&str>) -> HttpReply {
        let headers = retry_after.map(|v| ("Retry-After".to_string(), v.to_string())).into_iter().collect();
        HttpReply { status, headers, body: String::new() }
    }

    #[test]
    fn a_429_says_how_long() {
        assert_eq!(asked_wait(&reply(429, Some("2"))), Some(Duration::from_secs(2)));
        assert_eq!(asked_wait(&reply(429, Some("Fri, 31 Dec 1999 23:59:59 GMT"))), Some(Duration::ZERO));
        assert_eq!(asked_wait(&reply(429, None)), None);
        assert_eq!(asked_wait(&reply(429, Some("soon"))), None);
        // only a 429 is retried
        assert_eq!(asked_wait(&reply(503, Some("2"))), None);
        assert_eq!(asked_wait(&reply(200, Some("2"))), None);
    }

    #[test]
    fn idempotent_methods_unless_told() {
        let none = Args::default();
        for (method, retried) in
            [("GET", true), ("HEAD", true), ("PUT", true), ("DELETE", true), ("POST", false), ("PATCH", false)]
        {
            assert_eq!(allowed(method, &none).ok(), Some(retried), "{method}");
        }
        let told = |b: bool| Args { named: vec![("retry".into(), Value::Bool(b))], ..Args::default() };
        assert_eq!(allowed("POST", &told(true)).ok(), Some(true));
        assert_eq!(allowed("GET", &told(false)).ok(), Some(false));
        let bad = Args { named: vec![("retry".into(), Value::Int(3))], ..Args::default() };
        assert!(allowed("GET", &bad).is_err());
    }
}
