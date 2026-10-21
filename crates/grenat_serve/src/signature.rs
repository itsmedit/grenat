//! Who may call: webhook signatures (the body's HMAC-SHA256 with a shared
//! secret) and bearer tokens, compared in constant time.
//!
//! - GitHub signs the body: `X-Hub-Signature-256: sha256=<hex>`.
//! - Slack signs `v0:<timestamp>:<body>`: `X-Slack-Signature: v0=<hex>`,
//!   with the time in `X-Slack-Request-Timestamp`; a request more than five
//!   minutes away from now is refused, so a captured one cannot be replayed
//!   later (and [`crate::replay::Replays`] refuses it within those minutes).
//! - Microsoft Teams (outgoing webhooks) signs the body with the security
//!   token, base64-decoded: `Authorization: HMAC <base64>`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// How far, in seconds, a Slack request's timestamp may be from now.
pub const SLACK_TOLERANCE: i64 = 5 * 60;

fn hmac(key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `sha256=<hex>`, as GitHub sends it (`X-Hub-Signature-256`).
pub fn github(secret: &str, body: &[u8]) -> String {
    format!("sha256={}", hex(&hmac(secret.as_bytes(), &[body])))
}

/// Whether `given` is the GitHub signature of `body`, compared in constant time.
pub fn github_valid(secret: &str, body: &[u8], given: &str) -> bool {
    same(&github(secret, body), given)
}

/// `v0=<hex>`, as Slack sends it (`X-Slack-Signature`) for a request sent
/// at `timestamp` (`X-Slack-Request-Timestamp`).
pub fn slack(secret: &str, timestamp: &str, body: &[u8]) -> String {
    format!("v0={}", hex(&hmac(secret.as_bytes(), &[b"v0:", timestamp.as_bytes(), b":", body])))
}

/// Why a Slack request is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackRefusal {
    /// A header missing, or a timestamp that is no number.
    Unsigned,
    /// Sent more than [`SLACK_TOLERANCE`] seconds away from `now`.
    Stale,
    /// The signature is not the body's.
    Forged,
}

impl std::fmt::Display for SlackRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SlackRefusal::Unsigned => "not signed",
            SlackRefusal::Stale => "signed more than five minutes away from now",
            SlackRefusal::Forged => "bad signature",
        })
    }
}

/// Checks a Slack request: its headers' `timestamp` and `signature`, its
/// `body`, at `now` (seconds since the epoch).
pub fn slack_check(
    secret: &str,
    timestamp: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
    now: i64,
) -> Result<(), SlackRefusal> {
    let (Some(timestamp), Some(signature)) = (timestamp, signature) else { return Err(SlackRefusal::Unsigned) };
    let Ok(sent) = timestamp.trim().parse::<i64>() else { return Err(SlackRefusal::Unsigned) };
    if now.abs_diff(sent) > SLACK_TOLERANCE.unsigned_abs() {
        return Err(SlackRefusal::Stale);
    }
    if !same(&slack(secret, timestamp.trim(), body), signature.trim()) {
        return Err(SlackRefusal::Forged);
    }
    Ok(())
}

/// The key of a Teams outgoing webhook: its security token, base64-decoded
/// (`None` when it is not base64).
pub fn teams_key(token: &str) -> Option<Vec<u8>> {
    BASE64.decode(token.trim()).ok().filter(|key| !key.is_empty())
}

/// `HMAC <base64>`, as Teams sends it (`Authorization`).
pub fn teams(key: &[u8], body: &[u8]) -> String {
    format!("HMAC {}", BASE64.encode(hmac(key, &[body])))
}

/// Whether an `Authorization` header carries the Teams signature of `body`.
pub fn teams_valid(key: &[u8], body: &[u8], authorization: Option<&str>) -> bool {
    authorization.is_some_and(|given| same(&teams(key, body), given.trim()))
}

/// Whether an `Authorization` header carries `Bearer <token>`.
pub fn bearer_valid(token: &str, authorization: Option<&str>) -> bool {
    let given = authorization.and_then(|a| a.strip_prefix("Bearer ")).unwrap_or_default();
    same(token, given.trim())
}

/// Equality whose time does not tell how much of a secret was guessed.
fn same(expected: &str, given: &str) -> bool {
    expected.len() == given.len() && expected.bytes().zip(given.bytes()).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_signatures() {
        // GitHub's documented example
        let sig = github("It's a Secret to Everybody", b"Hello, World!");
        assert_eq!(sig, "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17");
        assert!(github_valid("It's a Secret to Everybody", b"Hello, World!", &sig));
        assert!(!github_valid("wrong", b"Hello, World!", &sig));
        assert!(!github_valid("It's a Secret to Everybody", b"Hello, World!", "sha256=00"));
    }

    /// Slack's documented example (docs.slack.dev, "Verifying requests from Slack").
    const SLACK_SECRET: &str = "8f742231b10e8888abcd99yyyzzz85a5";
    const SLACK_TIME: &str = "1531420618";
    const SLACK_BODY: &str = "token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow&channel_id=G8PSS9T3V&channel_name=foobar&user_id=U2CERLKJA&user_name=roadrunner&command=%2Fwebhook-collect&text=&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J%2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN&trigger_id=398738663015.47445629121.803a0bc887a14d10d2c447fce8b6703c";
    const SLACK_SIGNATURE: &str = "v0=a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503";

    #[test]
    fn slack_signatures() {
        assert_eq!(slack(SLACK_SECRET, SLACK_TIME, SLACK_BODY.as_bytes()), SLACK_SIGNATURE);
        let now = 1531420618;
        let check = |secret: &str, time: Option<&str>, sig: Option<&str>, body: &str, now: i64| {
            slack_check(secret, time, sig, body.as_bytes(), now)
        };
        assert_eq!(check(SLACK_SECRET, Some(SLACK_TIME), Some(SLACK_SIGNATURE), SLACK_BODY, now), Ok(()));
        // within five minutes, either way
        assert_eq!(check(SLACK_SECRET, Some(SLACK_TIME), Some(SLACK_SIGNATURE), SLACK_BODY, now + 300), Ok(()));
        assert_eq!(check(SLACK_SECRET, Some(SLACK_TIME), Some(SLACK_SIGNATURE), SLACK_BODY, now - 300), Ok(()));
        let stale = Err(SlackRefusal::Stale);
        assert_eq!(check(SLACK_SECRET, Some(SLACK_TIME), Some(SLACK_SIGNATURE), SLACK_BODY, now + 301), stale);
        assert_eq!(check(SLACK_SECRET, Some(SLACK_TIME), Some(SLACK_SIGNATURE), SLACK_BODY, now - 301), stale);
        let forged = Err(SlackRefusal::Forged);
        assert_eq!(check("wrong", Some(SLACK_TIME), Some(SLACK_SIGNATURE), SLACK_BODY, now), forged);
        assert_eq!(check(SLACK_SECRET, Some(SLACK_TIME), Some(SLACK_SIGNATURE), "token=x", now), forged);
        // the timestamp is signed: another one, even close, is another signature
        assert_eq!(check(SLACK_SECRET, Some("1531420619"), Some(SLACK_SIGNATURE), SLACK_BODY, now), forged);
        let unsigned = Err(SlackRefusal::Unsigned);
        assert_eq!(check(SLACK_SECRET, None, Some(SLACK_SIGNATURE), SLACK_BODY, now), unsigned);
        assert_eq!(check(SLACK_SECRET, Some(SLACK_TIME), None, SLACK_BODY, now), unsigned);
        assert_eq!(check(SLACK_SECRET, Some("soon"), Some(SLACK_SIGNATURE), SLACK_BODY, now), unsigned);
        // the farthest timestamps are stale, never an overflow
        for time in [i64::MIN.to_string(), i64::MAX.to_string()] {
            assert_eq!(check(SLACK_SECRET, Some(&time), Some(SLACK_SIGNATURE), SLACK_BODY, now), stale);
        }
    }

    #[test]
    fn teams_signatures() {
        let key = teams_key("vqF0En+Z0ucuRTM/01o2GuhMH3hKKk/N2bOmlM31zaA=").unwrap();
        assert_eq!(key.len(), 32);
        let body = br#"{"type":"message","text":"<at>Ops</at> status"}"#;
        let sig = teams(&key, body);
        // `openssl dgst -sha256 -mac HMAC -macopt hexkey:<key> | base64`
        assert_eq!(sig, "HMAC OIJ6946JL9nm4ZrQN2ybJzfgzcLGtvxwtqyA4vkmk+s=");
        assert!(teams_valid(&key, body, Some(&sig)));
        assert!(!teams_valid(&key, b"{}", Some(&sig)));
        assert!(!teams_valid(&teams_key("c2VjcmV0").unwrap(), body, Some(&sig)));
        assert!(!teams_valid(&key, body, None));
        assert!(!teams_valid(&key, body, Some("Bearer x")));
        assert_eq!(teams_key("not base64!"), None);
        assert_eq!(teams_key(""), None);
    }

    #[test]
    fn bearer_tokens() {
        assert!(bearer_valid("s3cret", Some("Bearer s3cret")));
        assert!(!bearer_valid("s3cret", Some("Bearer s3cre")));
        assert!(!bearer_valid("s3cret", Some("s3cret")));
        assert!(!bearer_valid("s3cret", None));
    }
}
