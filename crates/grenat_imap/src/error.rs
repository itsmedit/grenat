//! The one error of this crate: a kind a caller can branch on, and a
//! message fit for a user. Messages never carry the password or the token.

use std::fmt;

/// What went wrong, broadly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The mailbox URL or the options cannot describe a connection.
    Url,
    /// A connection without TLS, refused (or a server that offers no TLS).
    Insecure,
    /// The server could not be reached, or the connection broke.
    Connect,
    /// The server did not answer in time.
    Timeout,
    /// The TLS handshake failed (an unknown certificate, a wrong host name…).
    Tls,
    /// The server refused the credentials.
    Auth,
    /// The server refused a command (a folder that does not exist…).
    Refused,
    /// The folder was recreated: the identifiers of its messages changed.
    UidValidity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

pub type Result<T> = std::result::Result<T, Error>;

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

    /// Whether connecting again may help: the connection broke or went silent.
    pub fn retryable(&self) -> bool {
        matches!(self.kind, ErrorKind::Connect | ErrorKind::Timeout)
    }

    /// An error of the IMAP library, while doing `what`.
    pub(crate) fn imap(what: &str, error: imap::Error) -> Error {
        let kind = match &error {
            imap::Error::No(_) | imap::Error::Bad(_) => ErrorKind::Refused,
            // a line break in a name or a password: it would end the command
            imap::Error::Validate(_) => ErrorKind::Url,
            imap::Error::Io(io) => io_kind(io),
            // the connection broke, or the conversation lost its thread
            _ => ErrorKind::Connect,
        };
        Error::new(kind, format!("{what}: {}", one_line(&error.to_string())))
    }

    /// An error of the socket, while doing `what`.
    pub(crate) fn io(what: &str, error: &std::io::Error) -> Error {
        Error::new(io_kind(error), format!("{what}: {error}"))
    }

    /// The same error, `secret` replaced wherever it shows (a server could echo it).
    pub(crate) fn hiding(self, secret: &str) -> Error {
        if secret.is_empty() || !self.message.contains(secret) {
            return self;
        }
        Error::new(self.kind, self.message.replace(secret, "[hidden]"))
    }
}

/// A read or a write that waited longer than the socket allows is a timeout
/// (`WouldBlock` on Unix, `TimedOut` on Windows); anything else, a broken connection.
fn io_kind(error: &std::io::Error) -> ErrorKind {
    match error.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => ErrorKind::Timeout,
        _ => ErrorKind::Connect,
    }
}

/// What a server said, on one line (it decides what it sends).
fn one_line(text: &str) -> String {
    text.split(['\r', '\n']).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_message() {
        let e = Error::new(ErrorKind::Auth, "refused");
        assert_eq!((e.kind(), e.message(), e.to_string().as_str()), (ErrorKind::Auth, "refused", "refused"));
        assert!(!e.retryable());
    }

    #[test]
    fn socket_errors() {
        let silent = Error::io("reading", &std::io::Error::from(std::io::ErrorKind::WouldBlock));
        assert_eq!(silent.kind(), ErrorKind::Timeout);
        assert!(silent.retryable());
        let broken = Error::imap("UID FETCH", imap::Error::ConnectionLost);
        assert_eq!(broken.kind(), ErrorKind::Connect);
        assert!(broken.retryable());
        let timed_out = Error::imap("UID FETCH", imap::Error::Io(std::io::ErrorKind::TimedOut.into()));
        assert_eq!(timed_out.kind(), ErrorKind::Timeout);
    }

    #[test]
    fn secrets_are_hidden() {
        let e = Error::new(ErrorKind::Auth, "LOGIN u s3cret: refused\r\n").hiding("s3cret");
        assert_eq!(e.message(), "LOGIN u [hidden]: refused\r\n");
        assert_eq!(e.kind(), ErrorKind::Auth);
        // nothing to hide
        assert_eq!(Error::new(ErrorKind::Auth, "x").hiding("").message(), "x");
    }
}
