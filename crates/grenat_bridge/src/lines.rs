//! The lines a server writes, as text: a byte that is not UTF-8 is
//! replaced, and never stops the reading — a pipe no longer read would
//! break the server's next write (`Broken pipe`), and lose its log.

use std::io::{BufRead, BufReader, Read};

/// The lines of `reader`, without their `\n` (or `\r\n`), until its end.
pub fn lossy_lines(reader: impl Read) -> impl Iterator<Item = String> {
    let mut reader = BufReader::new(reader);
    std::iter::from_fn(move || {
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                if line.ends_with(b"\n") {
                    line.pop();
                    if line.ends_with(b"\r") {
                        line.pop();
                    }
                }
                Some(String::from_utf8_lossy(&line).into_owned())
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_go_on_after_bytes_that_are_not_utf8() {
        let bytes: &[u8] = b"caf\xe9\nplain\r\n\xff\xfe\nlast";
        let lines: Vec<String> = lossy_lines(bytes).collect();
        assert_eq!(lines, ["caf\u{fffd}", "plain", "\u{fffd}\u{fffd}", "last"]);
        assert_eq!(lossy_lines(&b""[..]).count(), 0);
    }
}
