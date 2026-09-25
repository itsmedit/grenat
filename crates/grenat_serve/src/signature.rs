//! Who may call: webhook signatures (the body's HMAC-SHA256 with a shared
//! secret) and bearer tokens, compared in constant time.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// `sha256=<hex>`, as GitHub sends it (`X-Hub-Signature-256`).
pub fn github(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    let hex: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

/// Whether `given` is the GitHub signature of `body`, compared in constant time.
pub fn github_valid(secret: &str, body: &[u8], given: &str) -> bool {
    same(&github(secret, body), given)
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

    #[test]
    fn bearer_tokens() {
        assert!(bearer_valid("s3cret", Some("Bearer s3cret")));
        assert!(!bearer_valid("s3cret", Some("Bearer s3cre")));
        assert!(!bearer_valid("s3cret", Some("s3cret")));
        assert!(!bearer_valid("s3cret", None));
    }
}
