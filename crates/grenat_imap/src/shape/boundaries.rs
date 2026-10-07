//! How many MIME parts a message's bytes can hold, counted without parsing
//! them: every part starts with `--` and its multipart's boundary, which a
//! `Content-Type` field declares (`boundary=…`).
//!
//! The count errs on the side of too many: a boundary is read up to its
//! first unusual character (a prefix of it matches wherever it does), and a
//! boundary written in a form not read here (RFC 2231's `boundary*0=`, an
//! encoded word, a comment) makes every `--` count.

use std::collections::HashMap;

/// At most this many multiparts, each with its boundary, are counted.
pub(crate) const MAX_BOUNDARIES: usize = 100;

/// The longest boundary RFC 2046 allows: what is read of one.
const MAX_BOUNDARY: usize = 70;

/// The boundaries declared in `Content-Type` values.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Boundaries<'a> {
    /// Each boundary, or a prefix of it.
    known: Vec<&'a [u8]>,
    /// A boundary was declared in a form not read: any `--` may start a part.
    unknown: bool,
}

impl<'a> Boundaries<'a> {
    /// Adds the boundaries declared in a `Content-Type` value.
    pub(crate) fn declared_in(&mut self, value: &'a [u8]) {
        let mut rest = value;
        while let Some(at) = position(rest, b"boundary") {
            let after = &rest[at + b"boundary".len()..];
            match boundary(after) {
                Declared::Value(prefix) => self.known.push(prefix),
                Declared::Unread => self.unknown = true,
                Declared::None => {}
            }
            rest = after;
        }
    }

    /// The parts `raw` can hold (the message itself is one); `None` when
    /// it declares more than [`MAX_BOUNDARIES`] boundaries.
    pub(crate) fn parts(&self, raw: &[u8]) -> Option<usize> {
        let mut known = self.known.clone();
        known.sort_unstable();
        known.dedup();
        if known.len() > MAX_BOUNDARIES {
            return None;
        }
        // a boundary that starts with another one is counted with it
        let shortest: Vec<&[u8]> =
            known.iter().filter(|b| !known.iter().any(|o| o.len() < b.len() && b.starts_with(o))).copied().collect();
        let mut by_first: HashMap<u8, Vec<&[u8]>> = HashMap::new();
        for b in shortest {
            by_first.entry(b[0]).or_default().push(b);
        }
        let dashes = raw.windows(2).enumerate().filter(|(_, w)| w == b"--").map(|(at, _)| at + 2);
        let starts = if self.unknown {
            dashes.count()
        } else {
            dashes
                .filter(|&at| {
                    let after = &raw[at..];
                    after
                        .first()
                        .and_then(|b| by_first.get(b))
                        .is_some_and(|bs| bs.iter().any(|b| after.starts_with(b)))
                })
                .count()
        };
        Some(starts + 1)
    }
}

enum Declared<'a> {
    Value(&'a [u8]),
    Unread,
    None,
}

/// What follows the word `boundary` in a `Content-Type` value: `=` then
/// a value, quoted or not; `*` (RFC 2231) is not read; anything else is
/// not an attribute.
fn boundary(after: &[u8]) -> Declared<'_> {
    let after = trim_start(after);
    match after.first() {
        Some(b'=') => {}
        Some(b'*') => return Declared::Unread,
        _ => return Declared::None,
    }
    let value = trim_start(&after[1..]);
    let value = value.strip_prefix(b"\"").unwrap_or(value);
    let end = (0..value.len()).find(|&i| ends_a_prefix(value, i)).unwrap_or(value.len());
    if end == 0 { Declared::Unread } else { Declared::Value(&value[..end.min(MAX_BOUNDARY)]) }
}

/// Where a boundary's prefix stops: white space, a delimiter, a quote, an
/// escape, a comment, an encoded word.
fn ends_a_prefix(value: &[u8], i: usize) -> bool {
    match value[i] {
        b';' | b'"' | b'\\' | b'(' | b')' => true,
        b'=' => value.get(i + 1) == Some(&b'?'),
        b => b.is_ascii_whitespace(),
    }
}

fn trim_start(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    &bytes[start..]
}

/// Where `needle` (lowercase) first appears in `haystack`, ignoring case.
fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(values: &[&'static str]) -> Boundaries<'static> {
        let mut boundaries = Boundaries::default();
        for value in values {
            boundaries.declared_in(value.as_bytes());
        }
        boundaries
    }

    #[test]
    fn boundaries_quoted_or_not() {
        let b = declared(&[" multipart/mixed; boundary=\"outer\"", " multipart/related; BOUNDARY = ----=_Part_1 ;x=y"]);
        assert_eq!(b.known, [&b"outer"[..], b"----=_Part_1"]);
        assert!(!b.unknown);
        // RFC 2231, an encoded word, an empty value: not read
        for value in ["a/b; boundary*0=x", "a/b; boundary=\"=?utf-8?q?x?=\"", "a/b; boundary=\"\""] {
            assert!(declared(&[value]).unknown, "{value}");
        }
        // a word, not an attribute
        assert_eq!(declared(&["a/b; name=\"boundary.txt\""]), Boundaries::default());
    }

    #[test]
    fn parts_are_counted_by_their_boundary() {
        let raw = b"--x\r\nA\r\n--y\r\n--x\r\nB --xyz -- --x--\r\n";
        assert_eq!(declared(&["; boundary=x"]).parts(raw), Some(5));
        // `xyz` starts with `x`: counted once
        assert_eq!(declared(&["; boundary=x", "; boundary=xyz"]).parts(raw), Some(5));
        assert_eq!(declared(&["; boundary=y"]).parts(raw), Some(2));
        // a boundary not read: every `--`
        assert_eq!(declared(&["; boundary*=x"]).parts(raw), Some(8));
        let many: Vec<String> = (0..=MAX_BOUNDARIES).map(|i| format!("; boundary=b{i}")).collect();
        let mut boundaries = Boundaries::default();
        many.iter().for_each(|v| boundaries.declared_in(v.as_bytes()));
        assert_eq!(boundaries.parts(raw), None);
    }
}
