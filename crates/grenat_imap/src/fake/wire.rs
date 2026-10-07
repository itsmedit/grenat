//! The fake server's side of the wire: a connection (clear or TLS, which
//! STARTTLS turns one into the other), its lines, the words of a command.

use std::io::{Read, Write};
use std::net::TcpStream;

use rustls::{ServerConnection, StreamOwned};

pub(crate) enum Conn {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ServerConnection, TcpStream>>),
    Closed,
}

impl Conn {
    pub(crate) fn is_tls(&self) -> bool {
        matches!(self, Conn::Tls(_))
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(tcp) => tcp.read(buf),
            Conn::Tls(tls) => tls.read(buf),
            Conn::Closed => Ok(0),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(tcp) => tcp.write(buf),
            Conn::Tls(tls) => tls.write(buf),
            Conn::Closed => Err(std::io::ErrorKind::NotConnected.into()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(tcp) => tcp.flush(),
            Conn::Tls(tls) => tls.flush(),
            Conn::Closed => Ok(()),
        }
    }
}

/// Reads one line (without its CRLF), byte by byte: nothing is read ahead,
/// so that STARTTLS finds the TLS handshake where it begins.
pub(crate) fn read_line(conn: &mut Conn) -> Option<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match conn.read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => break,
            Ok(1) => line.push(byte[0]),
            _ => return None,
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

/// The words of a command: atoms, quoted strings (unescaped), and
/// parenthesized lists kept whole (`(UID RFC822.SIZE)`).
pub(crate) fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' => {
                chars.next();
            }
            '"' => {
                chars.next();
                let mut word = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => word.extend(chars.next()),
                        '"' => break,
                        c => word.push(c),
                    }
                }
                words.push(word);
            }
            '(' => {
                let mut word = String::new();
                let mut depth = 0;
                for c in chars.by_ref() {
                    word.push(c);
                    depth += match c {
                        '(' => 1,
                        ')' => -1,
                        _ => 0,
                    };
                    if depth == 0 {
                        break;
                    }
                }
                words.push(word);
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ' ' {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                words.push(word);
            }
        }
    }
    words
}

/// The UIDs a set names (`1,3:5`, `*` the highest), among `known`.
pub(crate) fn uid_set(set: &str, known: &[u32]) -> Vec<u32> {
    let highest = known.iter().copied().max().unwrap_or(0);
    let number = |s: &str| if s == "*" { Some(highest) } else { s.parse::<u32>().ok() };
    let mut uids = Vec::new();
    for range in set.split(',') {
        let (lo, hi) = match range.split_once(':') {
            Some((a, b)) => (number(a), number(b)),
            None => (number(range), number(range)),
        };
        if let (Some(a), Some(b)) = (lo, hi) {
            let (lo, hi) = (a.min(b), a.max(b));
            uids.extend(known.iter().copied().filter(|u| (lo..=hi).contains(u)));
        }
    }
    uids.sort_unstable();
    uids.dedup();
    uids
}

/// Standard base64, decoded; `None` if it is not.
pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for c in text.trim_end_matches('=').bytes() {
        let value = TABLE.iter().position(|t| *t == c)? as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_words() {
        assert_eq!(words(r#"A1 LOGIN "a@b.c" "p\"w\\d""#), ["A1", "LOGIN", "a@b.c", "p\"w\\d"]);
        assert_eq!(words("A2 UID FETCH 1:3 (UID RFC822.SIZE)"), ["A2", "UID", "FETCH", "1:3", "(UID RFC822.SIZE)"]);
        assert_eq!(uid_set("1,3:5,*", &[1, 2, 3, 4, 9]), [1, 3, 4, 9]);
        assert_eq!(base64_decode("dXNlcj0=").unwrap(), b"user=");
    }
}
