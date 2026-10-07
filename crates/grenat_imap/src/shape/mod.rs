//! A message's structure, read from its bytes before any MIME parser
//! builds it: how many messages it nests, how many parts it holds, and
//! whether it carries a message encoded in base64 or quoted-printable.
//!
//! `mail-parser` builds a message nested in another as a tree, and frees
//! and copies such trees by recursion: one email nesting messages deeply
//! enough (`message/rfc822` in `message/rfc822`…, a few bytes per level)
//! exhausts any thread's stack — and a stack overflow aborts the whole
//! process. It parses encoded nested messages again from their decoded
//! bytes, so their depth cannot be seen in the raw bytes at all. And each
//! part costs it far more memory than the bytes that declare it. So a
//! message is read here first, and refused ([`crate::Malformed`]) when it
//! nests more than [`MAX_NESTED`] messages, holds more than [`MAX_PARTS`]
//! parts, or carries an encoded message: RFC 2046 (section 5.2.1) allows
//! only `7bit`, `8bit` and `binary` for `message/rfc822`, and a digest's
//! parts are `message/rfc822` by default (section 5.1.5); RFC 6532 (section
//! 3.7) allows any encoding for `message/global`, refused all the same, as
//! its nesting cannot be checked before decoding. Such a message is handed
//! to a human.
//!
//! Every count errs on the side of too many (see [`fields`]): a message
//! refused here may be one a parser would have read, never the reverse.

mod boundaries;
mod fields;

use std::collections::BTreeMap;

use boundaries::Boundaries;
use fields::{Field, Name, contains};

/// Messages nested in a message (forwarded whole, or in a digest), at most.
pub const MAX_NESTED: usize = 100;
/// MIME parts in a message, at most: its attachments among them.
pub const MAX_PARTS: usize = 1000;

/// What a message's bytes declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    /// `Content-Type` fields that make a part a message: `message/rfc822`,
    /// `message/global`, or `multipart/digest` (whose parts are messages).
    pub nested: usize,
    /// The parts it can hold; `None` when it declares too many multiparts to count.
    pub parts: Option<usize>,
    /// A part may be a message encoded in base64 or quoted-printable.
    pub encoded_message: bool,
}

/// What `raw` declares.
pub(crate) fn of(raw: &[u8]) -> Shape {
    let fields = fields::fields(raw);
    let mut boundaries = Boundaries::default();
    let mut blocks: BTreeMap<usize, Block> = BTreeMap::new();
    let (mut nested, mut digest) = (0, false);
    for Field { name, block, starts, lines } in fields {
        let seen = blocks.entry(block).or_default();
        match name {
            Name::ContentType => {
                // the type and its subtype share a line (a fold drops the subtype)
                let is_message = lines.iter().any(|line| is_message(line));
                let is_digest = lines.iter().any(|line| contains(line, b"digest"));
                nested += usize::from(is_message || is_digest);
                digest |= is_digest;
                seen.message |= is_message;
                seen.types += 1;
                seen.other_types += usize::from(!is_message && starts.iter().all(|start| names_another_type(start)));
                lines.iter().for_each(|line| boundaries.declared_in(line));
            }
            Name::TransferEncoding => seen.encoded |= !is_identity(&lines),
        }
    }
    // a part without a type of its own is a message in a digest
    let encoded_message =
        blocks.values().any(|b| b.encoded && (b.message || (digest && (b.types == 0 || b.other_types < b.types))));
    Shape { nested, parts: boundaries.parts(raw), encoded_message }
}

/// The fields of one header block that matter here.
#[derive(Default)]
struct Block {
    /// A type that makes the part a message.
    message: bool,
    /// Its `Content-Type` fields, and those that clearly name another type.
    types: usize,
    other_types: usize,
    /// A transfer encoding other than none (`7bit`, `8bit`, `binary`).
    encoded: bool,
}

/// `message/rfc822` or `message/global`, however written.
fn is_message(value: &[u8]) -> bool {
    contains(value, b"message") && (contains(value, b"rfc822") || contains(value, b"global"))
}

/// A value that plainly starts with a type that is not `message`
/// (`text/plain; …`): a parser reads that type, so the part is no message.
fn names_another_type(value: &[u8]) -> bool {
    let value = value.trim_ascii_start();
    let token = value.iter().take_while(|b| b.is_ascii_alphanumeric() || b"-+.".contains(b)).count();
    token > 0 && value.get(token) == Some(&b'/') && !value[..token].eq_ignore_ascii_case(b"message")
}

/// `7bit`, `8bit` or `binary` (white space aside): the bytes as they are.
fn is_identity(lines: &[&[u8]]) -> bool {
    let mut value = Vec::new();
    for line in lines {
        value.extend(line.iter().filter(|b| !b.is_ascii_whitespace()));
        if value.len() > b"binary".len() {
            return false;
        }
    }
    [&b"7bit"[..], b"8bit", b"binary"].iter().any(|identity| value.eq_ignore_ascii_case(identity))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(raw: &str) -> Shape {
        of(raw.as_bytes())
    }

    #[test]
    fn an_ordinary_message() {
        let raw = "From: a@b.c\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nHi=21\r\n--b\r\nContent-Type: message/rfc822\r\nContent-Transfer-Encoding: 7bit\r\n\r\nFrom: c@d.e\r\n\r\nInner\r\n--b\r\nContent-Type: application/pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBE\r\n--b--\r\n";
        assert_eq!(shape(raw), Shape { nested: 1, parts: Some(5), encoded_message: false });
    }

    #[test]
    fn encoded_messages_are_seen() {
        let encoded = "Content-Type: multipart/mixed; boundary=b\n\n--b\nContent-Type: Message/RFC822\nContent-Transfer-Encoding: BASE64\n\nRnJvbTo=\n--b--\n";
        assert!(shape(encoded).encoded_message);
        // an encoding that is none
        assert!(!shape(&encoded.replace("BASE64", " 8bit ")).encoded_message);
        // an encoded word decodes to anything
        assert!(shape(&encoded.replace("BASE64", "=?us-ascii?q?base64?=")).encoded_message);
        // in a digest, a part without a type (or with an unreadable one) is a message
        let digest =
            "Content-Type: multipart/digest; boundary=b\n\n--b\nContent-Transfer-Encoding: base64\n\nRnJvbTo=\n--b--\n";
        assert!(shape(digest).encoded_message);
        assert!(shape(&digest.replace("--b\n", "--b\nContent-Type: (x)/y\n")).encoded_message);
        assert!(!shape(&digest.replace("--b\n", "--b\nContent-Type: application/pdf\n")).encoded_message);
        // elsewhere, an encoded part is no message
        assert!(!shape(&digest.replace("digest", "mixed")).encoded_message);
    }

    #[test]
    fn nested_messages_are_counted() {
        let raw = "From: e@v.il\r\n".to_string() + &"Content-Type: message/rfc822\r\n\r\nFrom: a@b.c\r\n".repeat(150);
        assert_eq!(shape(&raw).nested, 150);
        assert_eq!(shape("Content-Type: message/delivery-status\n\n").nested, 0);
        assert_eq!(shape("Content-Type: multipart/digest; boundary=x\n\n").nested, 1);
    }
}
