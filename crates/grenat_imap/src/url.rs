//! Where a mailbox is: `imaps://user:password@host[:port]/Folder`.
//!
//! - `imaps://` is TLS from the first byte (port 993 by default);
//!   `imap://` connects in clear text and requires STARTTLS before any
//!   credential is sent (port 143 by default).
//! - The user and the password are percent-decoded; the host is what
//!   follows the *last* `@`, so `imaps://support@acme.com:pw@imap.gmail.com`
//!   reads as the user `support@acme.com` (write `%40` for an `@` in a folder).
//! - The folder is the path, percent-decoded (`INBOX` when there is none).
//!
//! The password never appears in `Debug` output nor in an error.

use std::fmt;

use crate::error::{Error, ErrorKind, Result};

/// How the connection is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte (`imaps://`).
    Implicit,
    /// Clear text upgraded with STARTTLS before logging in (`imap://`).
    StartTls,
    /// No TLS at all: only on the loopback interface, and only when asked
    /// for ([`MailboxUrl::without_tls`]).
    Plain,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MailboxUrl {
    pub security: Security,
    pub host: String,
    pub port: u16,
    pub user: String,
    password: String,
    /// The folder, as its user names it (`INBOX`, `Support/Tickets`).
    pub folder: String,
}

impl MailboxUrl {
    pub fn parse(url: &str) -> Result<MailboxUrl> {
        let invalid = |why: &str| Error::new(ErrorKind::Url, format!("invalid IMAP URL: {why}"));
        let (security, rest) = if let Some(rest) = url.strip_prefix("imaps://") {
            (Security::Implicit, rest)
        } else if let Some(rest) = url.strip_prefix("imap://") {
            (Security::StartTls, rest)
        } else {
            return Err(invalid("it starts with imaps:// (or imap://, with STARTTLS)"));
        };
        let Some(at) = rest.rfind('@') else {
            return Err(invalid("it names a user and a password (imaps://user:password@host/INBOX)"));
        };
        let (userinfo, address) = (&rest[..at], &rest[at + 1..]);
        let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
        let (authority, path) = address.split_once('/').unwrap_or((address, ""));
        // `[::1]:993`: an IPv6 address is bracketed
        let (host, port) = match authority.strip_prefix('[') {
            Some(bracketed) => {
                let (host, after) = bracketed.split_once(']').ok_or_else(|| invalid("its host"))?;
                (host, after.strip_prefix(':'))
            }
            None => match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            },
        };
        let port = match port {
            Some(port) => port.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| invalid("its port"))?,
            None if security == Security::Implicit => 993,
            None => 143,
        };
        if host.is_empty() || host.contains(|c: char| c.is_whitespace() || c == '?' || c == '#') {
            return Err(invalid("its host"));
        }
        let decode = |part: &str| percent_decode(part).ok_or_else(|| invalid("a percent-encoded part"));
        let folder = decode(path)?;
        Ok(MailboxUrl {
            security,
            host: host.to_string(),
            port,
            user: decode(user)?,
            password: decode(password)?,
            folder: if folder.is_empty() { "INBOX".to_string() } else { folder },
        })
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    /// The same mailbox reached without TLS: refused unless its host is
    /// the machine itself (a local server, a test).
    pub fn without_tls(mut self) -> Result<MailboxUrl> {
        if !is_loopback(&self.host) {
            return Err(Error::new(
                ErrorKind::Insecure,
                format!("IMAP without TLS is only for a server on this machine, not `{}`", self.host),
            ));
        }
        self.security = Security::Plain;
        Ok(self)
    }

    /// `host/folder`: how logs and events name the mailbox.
    pub fn label(&self) -> String {
        format!("{}/{}", self.host, self.folder)
    }
}

impl fmt::Debug for MailboxUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MailboxUrl")
            .field("security", &self.security)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &"[hidden]")
            .field("folder", &self.folder)
            .finish()
    }
}

/// `localhost`, `127.0.0.0/8` or `::1`.
pub fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `%40` → `@`; `None` for an incomplete escape or text that is not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let url = MailboxUrl::parse("imaps://support%40acme.com:app%20pw@imap.gmail.com/INBOX").unwrap();
        assert_eq!(url.security, Security::Implicit);
        assert_eq!((url.host.as_str(), url.port), ("imap.gmail.com", 993));
        assert_eq!((url.user.as_str(), url.password()), ("support@acme.com", "app pw"));
        assert_eq!(url.folder, "INBOX");
        assert_eq!(url.label(), "imap.gmail.com/INBOX");

        // the host follows the last `@`: an address as the user, unescaped
        let url = MailboxUrl::parse("imap://support@acme.com:p@ss:w0rd@mail.acme.com:1143").unwrap();
        assert_eq!(url.security, Security::StartTls);
        assert_eq!((url.user.as_str(), url.password()), ("support@acme.com", "p@ss:w0rd"));
        assert_eq!((url.host.as_str(), url.port, url.folder.as_str()), ("mail.acme.com", 1143, "INBOX"));

        let url = MailboxUrl::parse("imaps://u:p@[::1]:9993/Support/Caf%C3%A9").unwrap();
        assert_eq!((url.host.as_str(), url.port, url.folder.as_str()), ("::1", 9993, "Support/Café"));
        assert_eq!(MailboxUrl::parse("imap://u:p@h/").unwrap().port, 143);
    }

    #[test]
    fn invalid_urls_never_show_the_password() {
        for url in
            ["https://u:s3cret@h", "imaps://h/INBOX", "imaps://u:s3cret@h:x/", "imaps://u:s3cret@/", "imaps://u:%zz@h"]
        {
            let error = MailboxUrl::parse(url).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Url, "{url}");
            assert!(!error.message().contains("s3cret"), "{error}");
        }
        let url = MailboxUrl::parse("imaps://u:s3cret@h/INBOX").unwrap();
        assert!(!format!("{url:?}").contains("s3cret"));
    }

    #[test]
    fn plain_text_only_on_this_machine() {
        let local = MailboxUrl::parse("imap://u:p@127.0.0.1:1143/INBOX").unwrap();
        assert_eq!(local.without_tls().unwrap().security, Security::Plain);
        assert!(MailboxUrl::parse("imap://u:p@localhost/").unwrap().without_tls().is_ok());
        let remote = MailboxUrl::parse("imap://u:p@imap.acme.com/").unwrap().without_tls().unwrap_err();
        assert_eq!(remote.kind(), ErrorKind::Insecure);
    }
}
