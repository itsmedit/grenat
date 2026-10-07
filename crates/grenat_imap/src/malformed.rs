//! Why a message's bytes give no [`crate::Message`]: no header at all, or
//! a structure refused before it is parsed (see `shape`). A caller hands
//! such a message to a human rather than to a program.

use std::fmt;

use crate::shape::{MAX_NESTED, MAX_PARTS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malformed {
    /// Not a message: no header at all.
    NoHeader,
    /// It nests more than [`MAX_NESTED`] messages (this many, or fewer).
    TooNested(usize),
    /// It holds more than [`MAX_PARTS`] MIME parts (this many, or fewer;
    /// `None`: too many multiparts to count them).
    TooManyParts(Option<usize>),
    /// It carries a message encoded in base64 or quoted-printable.
    EncodedMessage,
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Malformed::NoHeader => f.write_str("it is not an email (no header)"),
            Malformed::TooNested(n) => {
                write!(f, "it nests up to {n} messages (forwarded or in digests), over the limit of {MAX_NESTED}")
            }
            Malformed::TooManyParts(Some(n)) => {
                write!(f, "it holds up to {n} MIME parts, over the limit of {MAX_PARTS}")
            }
            Malformed::TooManyParts(None) => {
                write!(f, "it holds too many multiparts, over the limit of {MAX_PARTS} parts")
            }
            Malformed::EncodedMessage => f.write_str(
                "it forwards a message encoded in base64 or quoted-printable, whose nesting cannot be checked before parsing",
            ),
        }
    }
}

impl std::error::Error for Malformed {}
