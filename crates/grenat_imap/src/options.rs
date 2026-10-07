//! How a mailbox is read: the certificates trusted, the time allowed, the
//! largest message fetched and how many are fetched at once.

use std::time::Duration;

use crate::tls::Trust;

/// 25 MiB: what Gmail and Microsoft 365 accept as a message (attachments
/// included, once encoded), so a message within the cap is any message a
/// person can send.
pub const MAX_MESSAGE_SIZE: u64 = 25 * 1024 * 1024;

/// Messages per `UID FETCH`: enough to save round trips, few enough that a
/// connection that breaks loses little.
pub const BATCH: usize = 20;

#[derive(Clone)]
pub struct Options {
    /// Which certificates a server may present.
    pub trust: Trust,
    /// The time allowed to connect, then for each read and each write.
    pub timeout: Duration,
    /// A larger message (as the server counts it) is never downloaded:
    /// it is reported as [`crate::Fetched::TooLarge`]. A batch never holds
    /// more than this either, so memory stays bounded by it.
    pub max_message_size: u64,
    /// At most this many messages per `UID FETCH`.
    pub batch: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            trust: Trust::default(),
            timeout: Duration::from_secs(60),
            max_message_size: MAX_MESSAGE_SIZE,
            batch: BATCH,
        }
    }
}
