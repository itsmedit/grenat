//! `grenat serve` reading the mailboxes of `on_email`, each on a task of
//! its own: at the start, then every `every:`, the unseen messages are
//! read (left unseen while read), each handed to the handler, oldest
//! first, then marked seen — or moved to `move_to:`.
//!
//! A handler that raises leaves its message unseen: it is tried again at
//! the next reads, [`MAX_ATTEMPTS`] times in all (counted in memory, per
//! UID and UIDVALIDITY), then flagged — and marked seen — for a human to
//! look at; a message too large to download, or that is no message or a
//! malformed one (see `grenat_imap::Malformed`), is flagged at once, and so
//! is a handled message the server refuses to move (it stays in its folder,
//! the read goes on with the next one). Each failure is an event (`grenat
//! console`) and a line on standard error; a mailbox that cannot be read is
//! too (its event recorded once until the error changes), and is read again
//! at the next turn. Nothing stops the server. Logs and events name a
//! mailbox by its host and folder: never its user, password or token.

use std::collections::HashMap;
use std::time::Duration;

use grenat_imap::{ErrorKind, Fetched, Inbox, Message, Trust};

use crate::RuntimeError;
use crate::builtins::email_value;
use crate::prelude::*;

/// Runs of a handler on one message before it is given up (flagged).
pub(crate) const MAX_ATTEMPTS: u32 = 3;

/// The failed runs of the messages still unseen, by UIDVALIDITY and UID.
#[derive(Default)]
pub(crate) struct Attempts(HashMap<(u32, u32), u32>);

impl Attempts {
    /// Counts a failed run of message `uid`: whether it is given up now.
    pub(crate) fn failed(&mut self, uid_validity: u32, uid: u32) -> bool {
        let count = self.0.entry((uid_validity, uid)).or_default();
        *count += 1;
        if *count >= MAX_ATTEMPTS {
            self.0.remove(&(uid_validity, uid));
            return true;
        }
        false
    }

    /// The message was handled: its failures are forgotten.
    pub(crate) fn forget(&mut self, uid_validity: u32, uid: u32) {
        self.0.remove(&(uid_validity, uid));
    }
}

/// What a mailbox's task keeps between two reads.
struct Watch {
    inbox: Inbox,
    attempts: Attempts,
    /// The last error reading the mailbox, recorded as an event once.
    last_error: Option<String>,
}

impl<'p> Interp<'p> {
    /// Reads mailbox `i` until the process ends; a failed read is
    /// reported, never fatal.
    pub(crate) fn watch_mailbox(&mut self, i: usize) {
        let (url, every, label) = {
            let mailboxes = self.mailboxes.borrow();
            let mailbox = &mailboxes[i];
            (mailbox.url.clone(), mailbox.every, mailbox.label())
        };
        // a stand-in URL exists in tests only, which never serve
        let Some(url) = url else { return };
        let options = self.imap_options();
        let mut watch =
            Watch { inbox: Inbox::new(url.clone(), options.clone()), attempts: Attempts::default(), last_error: None };
        loop {
            match self.read_mailbox(i, &mut watch) {
                Ok(()) => watch.last_error = None,
                Err(error) => {
                    self.write_err(&format!("[imap] {label}: {}: {}\n", error.ty, error.message));
                    if watch.last_error.as_ref() != Some(&error.message) {
                        self.record_event("email", &label, &error);
                    }
                    watch.last_error = Some(error.message);
                    // whatever state the connection is in, the next read opens a new one
                    watch.inbox = Inbox::new(url.clone(), options.clone());
                }
            }
            grenat_green::sleep(Duration::from_secs_f64(every));
        }
    }

    /// How mailboxes are reached: Mozilla's roots, and the certificates
    /// the options add.
    fn imap_options(&self) -> grenat_imap::Options {
        let trust = self.trusted_certificates.iter().fold(Trust::default(), |trust, der| trust.with(der.clone()));
        grenat_imap::Options { trust, ..grenat_imap::Options::default() }
    }

    /// One read: the unseen messages, each handled, then seen or moved.
    fn read_mailbox(&mut self, i: usize, watch: &mut Watch) -> Result<(), RuntimeError> {
        let (block, move_to, token, label) = {
            let mailboxes = self.mailboxes.borrow();
            let m = &mailboxes[i];
            (m.block.clone(), m.move_to.clone(), m.token.clone(), m.label())
        };
        if let Some(function) = token {
            let token = self.access_token(&function).map_err(|ctrl| self.runtime_error(ctrl))?;
            watch.inbox.set_token(Some(token));
        }
        let inbox = &mut watch.inbox;
        let (validity, unseen) =
            grenat_green::blocking(|| inbox.with(|m| Ok((m.uid_validity(), m.unseen()?)))).map_err(imap_error)?;
        if unseen.is_empty() {
            return Ok(());
        }
        self.write_err(&format!("[imap] {label}: {} new\n", unseen.len()));
        for chunk in unseen.chunks(grenat_imap::BATCH) {
            let inbox = &mut watch.inbox;
            let fetched =
                grenat_green::blocking(|| inbox.with_uids(validity, |m| m.fetch(chunk))).map_err(imap_error)?;
            for message in fetched {
                let uid = message.uid();
                let subject = format!("{label} (UID {uid})");
                let parsed = match message {
                    Fetched::Message { raw, .. } => Message::parse(&raw).map_err(|why| why.to_string()),
                    Fetched::TooLarge { size, .. } => {
                        Err(format!("it is {size} bytes, over the limit: never downloaded"))
                    }
                };
                let message = match parsed {
                    Ok(message) => message,
                    Err(why) => {
                        self.give_up(watch, validity, uid, &subject, email_error(why))?;
                        continue;
                    }
                };
                match self.call_block(&block, vec![email_value(&message)]) {
                    Ok(_) => {
                        watch.attempts.forget(validity, uid);
                        let inbox = &mut watch.inbox;
                        let done = grenat_green::blocking(|| {
                            inbox.with_uids(validity, |m| match &move_to {
                                Some(folder) => m.move_to(uid, folder),
                                None => m.mark_seen(uid),
                            })
                        });
                        match done {
                            Ok(()) => {}
                            // handled, but not filed (a folder missing, full…): for a human
                            Err(e) if e.kind() == ErrorKind::Refused => {
                                self.give_up(watch, validity, uid, &subject, imap_error(e))?;
                            }
                            Err(e) => return Err(imap_error(e)),
                        }
                    }
                    Err(ctrl) => {
                        let error = self.runtime_error(ctrl);
                        self.write_err(&format!("[imap] {subject}: {}: {}\n", error.ty, error.message));
                        self.record_event("email", &subject, &error);
                        if watch.attempts.failed(validity, uid) {
                            let why = format!("its handler failed {MAX_ATTEMPTS} times");
                            self.give_up(watch, validity, uid, &subject, email_error(why))?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Flags a message no one will handle, for a human to look at.
    fn give_up(
        &mut self,
        watch: &mut Watch,
        validity: u32,
        uid: u32,
        subject: &str,
        error: RuntimeError,
    ) -> Result<(), RuntimeError> {
        self.write_err(&format!("[imap] {subject}: given up ({}): flagged\n", error.message));
        self.record_event("email", subject, &error);
        let inbox = &mut watch.inbox;
        grenat_green::blocking(|| inbox.with_uids(validity, |m| m.mark_flagged(uid))).map_err(imap_error)
    }

    /// The OAuth 2.0 access token the function `name` gives (a `String`
    /// or a `Secret`; nothing untrusted reaches the server).
    fn access_token(&mut self, name: &str) -> Result<String, Ctrl<'p>> {
        let Some(def) = self.fns.get(name).copied() else {
            return raise("NameError", format!("`on_email` takes `token:` a function: no function `{name}`"));
        };
        let token = self.call_fn(def, Args::default(), None)?;
        if token.contains_taint() {
            return raise("TaintError", format!("the token `{name}` gives is untrusted: validate it before use"));
        }
        match token.untainted() {
            Value::Str(_) | Value::Secret(_) => Ok(token.reveal()),
            other => {
                raise("TypeError", format!("`{name}` gives a token: a string or a secret, got {}", other.type_name()))
            }
        }
    }
}

fn imap_error(error: grenat_imap::Error) -> RuntimeError {
    RuntimeError { ty: "ImapError".into(), message: error.message().to_string(), span: None, trace: Vec::new() }
}

fn email_error(message: String) -> RuntimeError {
    RuntimeError { ty: "EmailError".into(), message, span: None, trace: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_given_up_after_its_attempts() {
        let mut attempts = Attempts::default();
        assert!(!attempts.failed(7, 1));
        assert!(!attempts.failed(7, 1));
        // another folder's (or another UIDVALIDITY's) message counts apart
        assert!(!attempts.failed(8, 1));
        assert!(attempts.failed(7, 1));
        // given up: counted from the start, should it come back
        assert!(!attempts.failed(7, 1));
    }

    #[test]
    fn a_handled_message_is_forgotten() {
        let mut attempts = Attempts::default();
        assert!(!attempts.failed(7, 1));
        assert!(!attempts.failed(7, 1));
        attempts.forget(7, 1);
        assert!(!attempts.failed(7, 1));
        assert!(!attempts.failed(7, 1));
        assert!(attempts.failed(7, 1));
    }
}
