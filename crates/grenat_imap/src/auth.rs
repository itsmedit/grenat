//! Logging in, once the connection is protected: `LOGIN` with a password
//! (an app password, for Gmail), or SASL `XOAUTH2` with an OAuth 2.0
//! access token (Gmail, Microsoft 365, Outlook.com). Getting and
//! refreshing the token is the caller's business. A server that announces
//! `LOGINDISABLED` is never sent a password. A refusal names the host,
//! never the user, the password or the token.

use std::cell::Cell;
use std::fmt;

use crate::error::{Error, ErrorKind, Result};
use crate::tls::Stream;
use crate::url::MailboxUrl;

/// How to log in: the URL's password, or an OAuth 2.0 access token.
#[derive(Clone, Copy)]
pub enum Login<'a> {
    Password,
    OAuth2(&'a str),
}

impl fmt::Debug for Login<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Login::Password => f.write_str("Password"),
            Login::OAuth2(_) => f.debug_tuple("OAuth2").field(&"[hidden]").finish(),
        }
    }
}

impl Login<'_> {
    /// What an error must never show.
    pub(crate) fn secret<'a>(&'a self, url: &'a MailboxUrl) -> &'a str {
        match self {
            Login::Password => url.password(),
            Login::OAuth2(token) => token,
        }
    }
}

/// Logs `client` in as the URL's user; `offered` is what the server
/// announced once the connection was protected.
pub(crate) fn log_in(
    client: imap::Client<Stream>,
    url: &MailboxUrl,
    login: Login,
    offered: &imap::types::Capabilities,
) -> Result<imap::Session<Stream>> {
    let refused = |e: imap::Error| {
        // the user is no one's business either: logs and events name the host
        let e = Error::imap(&format!("logging in on {}", url.host), e);
        let e = if e.kind() == ErrorKind::Refused { Error::new(ErrorKind::Auth, e.message()) } else { e };
        e.hiding(login.secret(url)).hiding(&url.user)
    };
    match login {
        Login::Password if offered.has_str("LOGINDISABLED") => Err(Error::new(
            ErrorKind::Auth,
            format!("{} disables password logins (LOGINDISABLED): log in with a token", url.host),
        )),
        Login::Password => client.login(&url.user, url.password()).map_err(|(e, _)| refused(e)),
        Login::OAuth2(_) if !offered.has_str("AUTH=XOAUTH2") => {
            Err(Error::new(ErrorKind::Auth, format!("{} does not take OAuth 2.0 tokens (no AUTH=XOAUTH2)", url.host)))
        }
        Login::OAuth2(token) => {
            let sasl = XOAuth2 { user: &url.user, token, answered: Cell::new(false) };
            client.authenticate("XOAUTH2", &sasl).map_err(|(e, _)| refused(e))
        }
    }
}

/// SASL `XOAUTH2`: `user=…^Aauth=Bearer …^A^A`, base64-encoded by the
/// library. A server that refuses the token sends a challenge (its error,
/// in JSON) and waits for an empty answer before its `NO`.
struct XOAuth2<'a> {
    user: &'a str,
    token: &'a str,
    answered: Cell<bool>,
}

impl XOAuth2<'_> {
    fn initial(&self) -> String {
        format!("user={}\x01auth=Bearer {}\x01\x01", self.user, self.token)
    }
}

impl imap::Authenticator for XOAuth2<'_> {
    type Response = String;

    fn process(&self, _challenge: &[u8]) -> String {
        if self.answered.replace(true) { String::new() } else { self.initial() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imap::Authenticator;

    #[test]
    fn xoauth2_answers_once_then_acknowledges_the_error() {
        let sasl = XOAuth2 { user: "a@b.c", token: "ya29.t", answered: Cell::new(false) };
        assert_eq!(sasl.process(b""), "user=a@b.c\x01auth=Bearer ya29.t\x01\x01");
        assert_eq!(sasl.process(br#"{"status":"400"}"#), "");
    }

    #[test]
    fn a_token_never_shows_in_debug() {
        assert_eq!(format!("{:?}", Login::OAuth2("ya29.t")), r#"OAuth2("[hidden]")"#);
        assert_eq!(format!("{:?}", Login::Password), "Password");
    }
}
