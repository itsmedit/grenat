//! A mailbox read over IMAP, and its messages parsed: what wakes
//! `on_email`. Nothing here knows the Grenat language.
//!
//! **A blocking API.** [`Inbox`] connects when asked for something and
//! returns once the server has answered; callers (the interpreter's green
//! threads) call it inside a blocking section. Every read and write has a
//! timeout ([`Options::timeout`], 60 s by default). Pure Rust throughout:
//! `imap` for the protocol, `rustls` with `ring` and Mozilla's roots
//! (`webpki-roots`) for TLS, `mail-parser` for MIME — no system library.
//!
//! **TLS only.** [`MailboxUrl`] says how: `imaps://` (TLS from the first
//! byte, port 993, as RFC 8314 recommends) or `imap://` (STARTTLS, port
//! 143, required before any credential is sent; the capabilities heard
//! before TLS are forgotten). Clear text is for a server on this machine
//! only, and only when asked for ([`MailboxUrl::without_tls`]).
//!
//! **Logging in.** With the URL's password (`LOGIN`: an app password for
//! Gmail, where 2-Step Verification makes them available), or with an
//! OAuth 2.0 access token (SASL `XOAUTH2`, [`Login::OAuth2`]), which
//! Gmail and Microsoft 365 take — Microsoft 365 takes nothing else. The
//! token is the caller's to obtain and refresh. The password and the token
//! never appear in an error or in `Debug` output, nor the user in an error.
//!
//! **Messages are known by UID**, under the folder's UIDVALIDITY, and read
//! with `BODY.PEEK[]` in bounded batches ([`Mailbox::fetch`]; a message over
//! [`Options::max_message_size`], 25 MiB by default, is reported, never
//! downloaded): reading one never marks it seen — the caller decides when
//! ([`Mailbox::mark_seen`], [`Mailbox::mark_flagged`], [`Mailbox::move_to`]).
//! [`Message::parse`] makes a program's values of one — once its structure
//! is checked: a message nesting more than [`MAX_NESTED`] messages, holding
//! more than [`MAX_PARTS`] parts, or forwarding a message encoded in base64
//! or quoted-printable is [`Malformed`], never parsed (the MIME parser would
//! exhaust its stack or its memory on it).
//!
//! **Tests.** The `fake` feature adds the `fake` module: an IMAP server in
//! process (implicit TLS with a generated certificate, STARTTLS, or plain
//! on localhost), for the tests of this crate and of its users.

mod attachment;
mod auth;
mod error;
mod fetch;
mod inbox;
mod mailbox;
mod malformed;
mod message;
mod options;
mod shape;
mod starttls;
mod tls;
mod url;
mod utf7;

#[cfg(feature = "fake")]
pub mod fake;

pub use attachment::{Attachment, sanitize_name};
pub use auth::Login;
pub use error::{Error, ErrorKind, Result};
pub use fetch::Fetched;
pub use inbox::Inbox;
pub use mailbox::Mailbox;
pub use malformed::Malformed;
pub use message::Message;
pub use options::{BATCH, MAX_MESSAGE_SIZE, Options};
pub use shape::{MAX_NESTED, MAX_PARTS};
pub use tls::Trust;
pub use url::{MailboxUrl, Security, is_loopback};
pub use utf7::encode as encode_folder;
