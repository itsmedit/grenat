//! Whether a host name matches the host field of a known_hosts line, as
//! OpenSSH reads it: a comma-separated list of patterns (`*` and `?`
//! globs, `!` negations, `[host]:port` for a port other than 22), or one
//! hashed name `|1|salt|hash` (HMAC-SHA1 of the name, keyed by the salt).

use hmac::{Hmac, KeyInit, Mac};
use russh::keys::ssh_key::known_hosts::HostPatterns;
use sha1::Sha1;

/// How a host is named in known_hosts: `host`, or `[host]:port` when the
/// port is not 22. Lowercase, as OpenSSH writes and compares it.
pub fn host_name(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if port == 22 { host } else { format!("[{host}]:{port}") }
}

/// Whether `name` (a [`host_name`]) matches `patterns`. A matching negation
/// wins over any positive match.
pub fn matches(patterns: &HostPatterns, name: &str) -> bool {
    match patterns {
        HostPatterns::HashedName { salt, hash } => hashed_matches(salt, hash, name),
        HostPatterns::Patterns(patterns) => {
            let mut matched = false;
            for pattern in patterns {
                let pattern = pattern.to_ascii_lowercase();
                match pattern.strip_prefix('!') {
                    Some(negated) if glob(negated.as_bytes(), name.as_bytes()) => return false,
                    Some(_) => {}
                    None => matched |= glob(pattern.as_bytes(), name.as_bytes()),
                }
            }
            matched
        }
    }
}

fn hashed_matches(salt: &[u8], hash: &[u8; 20], name: &str) -> bool {
    let Ok(mac) = Hmac::<Sha1>::new_from_slice(salt) else {
        return false;
    };
    mac.chain_update(name.as_bytes()).verify_slice(hash).is_ok()
}

/// `*` matches any run of characters, `?` exactly one.
fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|skip| glob(rest, &text[skip..])),
        Some((b'?', rest)) => !text.is_empty() && glob(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && glob(rest, &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(s: &str) -> HostPatterns {
        s.parse().unwrap()
    }

    #[test]
    fn names() {
        assert_eq!(host_name("Example.COM", 22), "example.com");
        assert_eq!(host_name("10.0.0.1", 2222), "[10.0.0.1]:2222");
    }

    #[test]
    fn plain_lists_and_globs() {
        assert!(matches(&patterns("example.com"), "example.com"));
        assert!(matches(&patterns("other.org,Example.com"), "example.com"));
        assert!(!matches(&patterns("example.org"), "example.com"));
        assert!(matches(&patterns("*.example.com"), "db.example.com"));
        assert!(!matches(&patterns("*.example.com"), "example.com"));
        assert!(matches(&patterns("web?.example.com"), "web1.example.com"));
        assert!(!matches(&patterns("web?.example.com"), "web12.example.com"));
        assert!(matches(&patterns("[127.0.0.1]:2222"), "[127.0.0.1]:2222"));
        assert!(!matches(&patterns("127.0.0.1"), "[127.0.0.1]:2222"));
    }

    #[test]
    fn negations_win() {
        let p = patterns("*.example.com,!secret.example.com");
        assert!(matches(&p, "db.example.com"));
        assert!(!matches(&p, "secret.example.com"));
        assert!(!matches(&patterns("!example.com"), "example.com"));
        assert!(!matches(&patterns("!other.com"), "example.com"));
    }

    #[test]
    fn hashed_names() {
        // What `ssh-keygen -H` would write for `localhost` with this salt.
        let salt = [7u8; 20];
        let mut mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap();
        mac.update(b"localhost");
        let hash: [u8; 20] = mac.finalize().into_bytes().into();
        let hashed = HostPatterns::HashedName { salt: salt.to_vec(), hash };
        assert!(matches(&hashed, "localhost"));
        assert!(!matches(&hashed, "localhost2"));
        // And one written by OpenSSH itself, for `example.com`.
        let openssh = patterns("|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak=");
        assert!(matches(&openssh, "example.com"));
        assert!(!matches(&openssh, "example.org"));
    }
}
