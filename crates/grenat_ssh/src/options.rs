//! What a connection needs: who, where, with which credentials, how the
//! server's host key is trusted, through which proxy and within how long.
//! Secrets (key text, passphrase, password, proxy credentials) never show
//! in `Debug`.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::error::{Error, ErrorKind};

/// How the client proves who it is.
#[derive(Clone)]
pub enum Auth {
    /// A private key, as the text of its file (OpenSSH format; PEM and PuTTY
    /// are read too), and the passphrase if the key is encrypted.
    Key {
        text: String,
        passphrase: Option<String>,
    },
    Password(String),
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Auth::Key { passphrase, .. } => f
                .debug_struct("Key")
                .field("text", &"<secret>")
                .field("passphrase", &passphrase.as_ref().map(|_| "<secret>"))
                .finish(),
            Auth::Password(_) => f.debug_tuple("Password").field(&"<secret>").finish(),
        }
    }
}

/// How the server's host key is trusted. Nothing is ever accepted silently:
/// an unknown or changed key is an error naming the key offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownHosts {
    /// The key must be recorded for the host in this known_hosts file.
    File(PathBuf),
    /// The key must have this fingerprint (`SHA256:…`, as `ssh-keygen -l`
    /// prints it), checked out of band.
    Fingerprint(String),
    /// Trust on first use: a host not yet in this file has its key recorded
    /// there (and the session says so); a known host must offer its recorded
    /// key, and a changed key is refused as with [`KnownHosts::File`].
    RecordInto(PathBuf),
}

impl KnownHosts {
    /// `~/.ssh/known_hosts`.
    pub fn user_file() -> KnownHosts {
        KnownHosts::File(std::env::home_dir().map(|home| home.join(".ssh").join("known_hosts")).unwrap_or_default())
    }
}

impl Default for KnownHosts {
    fn default() -> KnownHosts {
        KnownHosts::user_file()
    }
}

/// `host:port`, with an IPv6 address in brackets.
pub fn target(host: &str, port: u16) -> String {
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

#[derive(Clone)]
pub struct Options {
    pub user: String,
    pub host: String,
    pub port: u16,
    pub auth: Auth,
    pub known_hosts: KnownHosts,
    /// `socks5://[user:password@]host[:port]` (`socks5h://` too; the proxy
    /// resolves the server's name either way).
    pub proxy: Option<String>,
    /// How long connecting may take: TCP (and the proxy), the SSH handshake
    /// and authentication; also the longest wait for an SFTP answer. `None`:
    /// no limit.
    pub timeout: Option<Duration>,
}

impl Options {
    /// Port 22, `~/.ssh/known_hosts`, no proxy, 30 seconds to connect.
    pub fn new(user: impl Into<String>, host: impl Into<String>, auth: Auth) -> Options {
        Options {
            user: user.into(),
            host: host.into(),
            port: 22,
            auth,
            known_hosts: KnownHosts::default(),
            proxy: None,
            timeout: Some(Duration::from_secs(30)),
        }
    }

    /// `host:port`, as messages name the server.
    pub fn target(&self) -> String {
        target(&self.host, self.port)
    }

    /// Refuses what cannot name a connection. The host also becomes part of
    /// a known_hosts line, so it may not carry spaces, commas or brackets.
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = |message: String| Err(Error::new(ErrorKind::Connect, message));
        if self.user.is_empty() {
            return invalid("an SSH connection needs a user".into());
        }
        if self.user.chars().any(|c| c.is_control()) {
            return invalid("the SSH user name cannot contain control characters".into());
        }
        if self.host.is_empty() {
            return invalid("an SSH connection needs a host".into());
        }
        if !self.host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%')) {
            return invalid(format!("`{}` is not a valid host name or address", self.host.escape_debug()));
        }
        if self.port == 0 {
            return invalid("the SSH port cannot be 0".into());
        }
        Ok(())
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("user", &self.user)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("auth", &self.auth)
            .field("known_hosts", &self.known_hosts)
            .field("proxy", &self.proxy.as_ref().map(|_| "<proxy>"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options::new("alice", "example.com", Auth::Password("hunter2".into()))
    }

    #[test]
    fn defaults() {
        let o = options();
        assert_eq!((o.port, o.proxy.clone(), o.timeout), (22, None, Some(Duration::from_secs(30))));
        match &o.known_hosts {
            KnownHosts::File(path) => assert!(path.ends_with(".ssh/known_hosts"), "{path:?}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(o.target(), "example.com:22");
        assert_eq!(Options { host: "::1".into(), ..o }.target(), "[::1]:22");
    }

    #[test]
    fn debug_hides_secrets() {
        let mut o = options();
        o.proxy = Some("socks5://bob:proxy-pass@proxy:1080".into());
        let shown = format!("{o:?}");
        assert!(!shown.contains("hunter2") && !shown.contains("proxy-pass"), "{shown}");
        o.auth = Auth::Key { text: "-----BEGIN OPENSSH PRIVATE KEY-----xyz".into(), passphrase: Some("pp".into()) };
        let shown = format!("{o:?}");
        assert!(!shown.contains("xyz") && !shown.contains("\"pp\""), "{shown}");
    }

    #[test]
    fn validation() {
        assert!(options().validate().is_ok());
        assert!(Options { host: "10.0.0.1".into(), ..options() }.validate().is_ok());
        assert!(Options { host: "fe80::1%en0".into(), ..options() }.validate().is_ok());
        for host in ["", "a b", "a,b", "[a]", "a\nb", "a*"] {
            let e = Options { host: host.into(), ..options() }.validate().unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Connect, "{host:?}");
        }
        assert!(Options { user: String::new(), ..options() }.validate().is_err());
        assert!(Options { user: "a\nb".into(), ..options() }.validate().is_err());
        assert!(Options { port: 0, ..options() }.validate().is_err());
    }
}
