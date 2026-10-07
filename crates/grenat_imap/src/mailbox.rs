//! A folder, open: logged in over TLS, selected, and what `on_email` asks
//! of it — the unseen messages, the messages whole, then seen, flagged or
//! moved. Messages are known by their UID (stable while the folder's
//! UIDVALIDITY is), never by their sequence number.
//!
//! The client speaks IMAP4rev1 (RFC 3501), which every server speaks, and
//! uses what a server announces beyond it: `MOVE` (RFC 6851) and `UIDPLUS`
//! (`UID EXPUNGE`, RFC 4315), both part of IMAP4rev2 (RFC 9051).
//! Credentials go out only once the connection is protected.

use crate::auth::{self, Login};
use crate::error::{Error, ErrorKind, Result};
use crate::fetch::{self, Fetched};
use crate::options::Options;
use crate::starttls;
use crate::tls::{self, Stream};
use crate::url::{MailboxUrl, Security};
use crate::utf7;

/// UIDs per `UID FETCH (RFC822.SIZE)`: a command line stays short.
const SIZES_PER_COMMAND: usize = 500;

pub struct Mailbox {
    session: imap::Session<Stream>,
    uid_validity: u32,
    can_move: bool,
    uidplus: bool,
    max_message_size: u64,
    batch: usize,
    /// What an error must never show.
    secret: String,
}

impl Mailbox {
    /// Connects, logs in and selects the URL's folder.
    pub fn open(url: &MailboxUrl, login: Login, options: &Options) -> Result<Mailbox> {
        let secret = login.secret(url).to_string();
        let plain = url.security == Security::Plain;
        let mut tcp = tls::connect(&url.host, url.port, options.timeout, plain)?;
        let mut client = match url.security {
            Security::Implicit => greeted(imap::Client::new(tls::secure(tcp, &url.host, &options.trust)?))?,
            Security::Plain => greeted(imap::Client::new(Stream::Plain(tcp)))?,
            Security::StartTls => {
                starttls::negotiate(&mut tcp, &url.host, options.timeout)?;
                // no greeting after STARTTLS
                imap::Client::new(tls::secure(tcp, &url.host, &options.trust)?)
            }
        };
        // what the server offers once the connection is protected (what it
        // said before is forgotten: an attacker could have written it)
        let offered = client.capabilities().map_err(|e| Error::imap("CAPABILITY", e))?;
        let mut session = auth::log_in(client, url, login, &offered)?;
        // servers announce more once logged in
        let offered = session.capabilities().map_err(|e| Error::imap("CAPABILITY", e).hiding(&secret))?;
        let rev2 = offered.has_str("IMAP4rev2");
        // folder names travel in modified UTF-7 (the library quotes them)
        let selected = session
            .select(utf7::encode(&url.folder))
            .map_err(|e| Error::imap(&format!("selecting the folder `{}`", url.folder), e).hiding(&secret))?;
        Ok(Mailbox {
            uid_validity: selected.uid_validity.unwrap_or(0),
            can_move: rev2 || offered.has_str("MOVE"),
            uidplus: rev2 || offered.has_str("UIDPLUS"),
            max_message_size: options.max_message_size,
            batch: options.batch,
            session,
            secret,
        })
    }

    /// Identifies the folder's UIDs: when it changes, they name other messages.
    pub fn uid_validity(&self) -> u32 {
        self.uid_validity
    }

    /// The UIDs of the messages neither seen nor deleted, oldest first.
    pub fn unseen(&mut self) -> Result<Vec<u32>> {
        let found = self.session.uid_search("UNSEEN UNDELETED").map_err(|e| self.error("UID SEARCH", e))?;
        let mut uids: Vec<u32> = found.into_iter().collect();
        uids.sort_unstable();
        Ok(uids)
    }

    /// The size of each message of `uids` (in octets, as the server counts
    /// it), in the order of `uids`; a message gone is left out.
    pub fn sizes(&mut self, uids: &[u32]) -> Result<Vec<(u32, u64)>> {
        let mut known = std::collections::HashMap::new();
        for chunk in uids.chunks(SIZES_PER_COMMAND) {
            let fetched = self
                .session
                .uid_fetch(fetch::uid_set(chunk), "(UID RFC822.SIZE)")
                .map_err(|e| self.error("UID FETCH", e))?;
            known.extend(fetched.iter().filter_map(|f| Some((f.uid?, u64::from(f.size?)))));
        }
        Ok(uids.iter().filter_map(|uid| Some((*uid, *known.get(uid)?))).collect())
    }

    /// The messages of `uids`, whole and left unseen, in the order of
    /// `uids`: fetched in batches ([`Options::batch`]), those over the cap
    /// ([`Options::max_message_size`]) reported rather than downloaded, and
    /// those gone left out. All the messages are held at once: ask for a
    /// batch's worth at a time to keep memory bounded.
    pub fn fetch(&mut self, uids: &[u32]) -> Result<Vec<Fetched>> {
        let sizes = self.sizes(uids)?;
        let (too_large, batches) = fetch::plan(&sizes, self.max_message_size, self.batch);
        let mut found: std::collections::HashMap<u32, Fetched> =
            too_large.into_iter().map(|(uid, size)| (uid, Fetched::TooLarge { uid, size })).collect();
        for batch in batches {
            let fetched = self
                .session
                .uid_fetch(fetch::uid_set(&batch), "(UID BODY.PEEK[])")
                .map_err(|e| self.error("UID FETCH", e))?;
            for message in fetched.iter() {
                let (Some(uid), Some(raw)) = (message.uid, message.body()) else { continue };
                // the size announced was a lie: the cap holds all the same
                let item = if raw.len() as u64 > self.max_message_size {
                    Fetched::TooLarge { uid, size: raw.len() as u64 }
                } else {
                    Fetched::Message { uid, raw: raw.to_vec() }
                };
                found.insert(uid, item);
            }
        }
        Ok(uids.iter().filter_map(|uid| found.remove(uid)).collect())
    }

    /// Marks the message seen.
    pub fn mark_seen(&mut self, uid: u32) -> Result<()> {
        self.store(uid, "+FLAGS.SILENT (\\Seen)")
    }

    /// Marks the message flagged — for a human to look at — and seen, so
    /// that it is not handled again as a new message.
    pub fn mark_flagged(&mut self, uid: u32) -> Result<()> {
        self.store(uid, "+FLAGS.SILENT (\\Seen \\Flagged)")
    }

    /// Marks the message seen, then moves it to `folder` (which must exist):
    /// `UID MOVE` where the server has it; else a copy, the original marked
    /// `\Deleted` and expunged — by `UID EXPUNGE` with `UIDPLUS`, otherwise
    /// by `EXPUNGE`, which also removes the folder's other messages already
    /// marked `\Deleted` (as their owner asked).
    pub fn move_to(&mut self, uid: u32, folder: &str) -> Result<()> {
        self.mark_seen(uid)?;
        let what = format!("moving a message to `{folder}`");
        if self.can_move {
            return self.session.uid_mv(uid.to_string(), utf7::encode(folder)).map_err(|e| self.error(&what, e));
        }
        // the library sends COPY's folder as given: quoted here
        self.session.uid_copy(uid.to_string(), utf7::quoted(folder)).map_err(|e| self.error(&what, e))?;
        self.store(uid, "+FLAGS.SILENT (\\Deleted)")?;
        if self.uidplus {
            self.session.uid_expunge(uid.to_string()).map(drop).map_err(|e| self.error("UID EXPUNGE", e))
        } else {
            self.session.expunge().map(drop).map_err(|e| self.error("EXPUNGE", e))
        }
    }

    /// Asks the server for nothing: whether the connection still works.
    pub fn noop(&mut self) -> Result<()> {
        self.session.noop().map_err(|e| self.error("NOOP", e))
    }

    /// Logs out (the connection is closed either way).
    pub fn logout(mut self) {
        let _ = self.session.logout();
    }

    fn store(&mut self, uid: u32, flags: &str) -> Result<()> {
        self.session.uid_store(uid.to_string(), flags).map(drop).map_err(|e| self.error("UID STORE", e))
    }

    fn error(&self, what: &str, e: imap::Error) -> Error {
        Error::imap(what, e).hiding(&self.secret)
    }
}

/// A client whose greeting was read: `* OK`, or an error.
fn greeted(mut client: imap::Client<Stream>) -> Result<imap::Client<Stream>> {
    let greeting = client.read_greeting().map_err(|e| Error::imap("reading the greeting", e))?;
    let text = String::from_utf8_lossy(&greeting);
    if !text.starts_with("* OK") {
        return Err(Error::new(ErrorKind::Refused, format!("the server does not welcome us: {}", text.trim())));
    }
    Ok(client)
}
