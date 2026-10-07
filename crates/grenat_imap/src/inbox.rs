//! A mailbox watched over time: opened when needed, opened again once
//! when the connection broke or went silent (a server's idle timeout while
//! a message was being handled), and never mistaken for another folder —
//! an operation on a UID says which UIDVALIDITY it was read under, and is
//! refused if the folder was recreated since, as the UID may name another
//! message now.
//!
//! An operation run again after a reconnection must bear it: reading,
//! flagging and `UID MOVE` do (a UID gone is no error); a `COPY` that
//! completed before the break is made again.

use crate::auth::Login;
use crate::error::{Error, ErrorKind, Result};
use crate::mailbox::Mailbox;
use crate::options::Options;
use crate::url::MailboxUrl;

pub struct Inbox {
    url: MailboxUrl,
    options: Options,
    token: Option<String>,
    open: Option<Mailbox>,
}

impl Inbox {
    pub fn new(url: MailboxUrl, options: Options) -> Inbox {
        Inbox { url, options, token: None, open: None }
    }

    pub fn url(&self) -> &MailboxUrl {
        &self.url
    }

    /// Logs in with an OAuth 2.0 access token from now on (`None`: the
    /// URL's password). The connection open, if any, stays.
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    /// Runs `op` on the folder, opened first if need be; a connection that
    /// broke is opened again, and `op` run again, once.
    pub fn with<T>(&mut self, op: impl FnMut(&mut Mailbox) -> Result<T>) -> Result<T> {
        self.run(None, op)
    }

    /// As [`Inbox::with`], for an operation on UIDs read while the folder's
    /// UIDVALIDITY was `uid_validity`: refused if it changed.
    pub fn with_uids<T>(&mut self, uid_validity: u32, op: impl FnMut(&mut Mailbox) -> Result<T>) -> Result<T> {
        self.run(Some(uid_validity), op)
    }

    /// Logs out; the next operation connects again.
    pub fn close(&mut self) {
        if let Some(mailbox) = self.open.take() {
            mailbox.logout();
        }
    }

    fn run<T>(&mut self, expected: Option<u32>, mut op: impl FnMut(&mut Mailbox) -> Result<T>) -> Result<T> {
        let mut retried = false;
        loop {
            let label = self.url.label();
            let mailbox = self.mailbox()?;
            if expected.is_some_and(|v| v != mailbox.uid_validity()) {
                return Err(Error::new(
                    ErrorKind::UidValidity,
                    format!("{label} was recreated (its UIDVALIDITY changed): its messages are new to us"),
                ));
            }
            match op(mailbox) {
                Err(e) if e.retryable() => {
                    self.open = None;
                    if retried {
                        return Err(e);
                    }
                    retried = true;
                }
                result => return result,
            }
        }
    }

    fn mailbox(&mut self) -> Result<&mut Mailbox> {
        if self.open.is_none() {
            let login = self.token.as_deref().map_or(Login::Password, Login::OAuth2);
            self.open = Some(Mailbox::open(&self.url, login, &self.options)?);
        }
        Ok(self.open.as_mut().expect("opened above"))
    }
}
