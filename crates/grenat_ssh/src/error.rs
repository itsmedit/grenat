//! The one error of this crate: a kind a program can branch on, and a
//! message fit for a user. Messages never carry key material, passphrases,
//! passwords or proxy credentials.

use std::fmt;

/// What went wrong, broadly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The server could not be reached, or the SSH connection broke; also
    /// options that cannot describe a connection.
    Connect,
    /// The server's host key is unknown, changed, revoked or not the expected one.
    HostKey,
    /// The server refused the credentials, or the private key cannot be read.
    Auth,
    /// A remote command could not be started.
    Command,
    /// An SFTP operation failed (on the remote side, or on the local file of a transfer).
    Sftp,
    /// The SOCKS5 proxy could not be reached or refused the connection.
    Proxy,
    /// Something took longer than allowed.
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error { kind, message: message.into() }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// What the SSH library reports once a connection exists: the connection
/// broke, or a keepalive went unanswered.
impl From<russh::Error> for Error {
    fn from(error: russh::Error) -> Error {
        match error {
            russh::Error::ConnectionTimeout | russh::Error::KeepaliveTimeout | russh::Error::InactivityTimeout => {
                Error::new(ErrorKind::Timeout, format!("the SSH connection timed out: {error}"))
            }
            russh::Error::Disconnect | russh::Error::HUP => {
                Error::new(ErrorKind::Connect, "the SSH server closed the connection")
            }
            other => Error::new(ErrorKind::Connect, format!("SSH connection error: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_message() {
        let e = Error::new(ErrorKind::Auth, "refused");
        assert_eq!((e.kind(), e.message(), e.to_string().as_str()), (ErrorKind::Auth, "refused", "refused"));
    }

    #[test]
    fn library_errors() {
        assert_eq!(Error::from(russh::Error::KeepaliveTimeout).kind(), ErrorKind::Timeout);
        assert_eq!(Error::from(russh::Error::HUP).message(), "the SSH server closed the connection");
        assert_eq!(Error::from(russh::Error::Kex).kind(), ErrorKind::Connect);
    }
}
