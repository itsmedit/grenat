//! Server-Sent Events, read as they arrive: the bytes of a stream cut into
//! events (`event:` and `data:` lines, a blank line ending each), whatever
//! pieces the network delivers them in — a line split between two reads, a
//! character whose bytes are, `\n`, `\r\n` or `\r` line ends.

use std::io::Read;

/// One event: its name (`message` when it has none) and its data, the
/// `data:` lines joined with `\n`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Event {
    pub name: String,
    pub data: String,
}

pub(crate) struct Events<R> {
    inner: R,
    /// Bytes read, not yet cut into lines.
    pending: Vec<u8>,
    ended: bool,
    /// The event being read.
    name: String,
    data: Vec<String>,
    /// The stream's first line may start with a byte order mark.
    first_line: bool,
}

impl<R: Read> Events<R> {
    pub(crate) fn new(inner: R) -> Self {
        Events { inner, pending: Vec::new(), ended: false, name: String::new(), data: Vec::new(), first_line: true }
    }

    /// The next whole event; `None` at the end of the stream (an event the
    /// stream cut short is dropped, as the format says).
    pub(crate) fn next_event(&mut self) -> Result<Option<Event>, String> {
        while let Some(line) = self.next_line()? {
            if line.is_empty() {
                if self.data.is_empty() {
                    self.name.clear();
                    continue;
                }
                let name = std::mem::take(&mut self.name);
                let data = std::mem::take(&mut self.data).join("\n");
                return Ok(Some(Event { name: if name.is_empty() { "message".into() } else { name }, data }));
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((&line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.name = value.to_string(),
                "data" => self.data.push(value.to_string()),
                // `id` and `retry` serve reconnections: not used here
                _ => {}
            }
        }
        Ok(None)
    }

    /// The next line, without its end; `None` once the stream has ended.
    fn next_line(&mut self) -> Result<Option<String>, String> {
        loop {
            if let Some(end) = self.pending.iter().position(|b| matches!(b, b'\n' | b'\r')) {
                // a `\r` last of what was read may be the start of a `\r\n`
                if self.pending[end] == b'\r' && end + 1 == self.pending.len() && !self.ended {
                    self.fill()?;
                    continue;
                }
                let skip = if self.pending[end] == b'\r' && self.pending.get(end + 1) == Some(&b'\n') { 2 } else { 1 };
                let bytes: Vec<u8> = self.pending.drain(..end + skip).take(end).collect();
                return Ok(Some(self.decode(bytes)));
            }
            if self.ended {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                // a last line without its end
                let bytes = std::mem::take(&mut self.pending);
                return Ok(Some(self.decode(bytes)));
            }
            self.fill()?;
        }
    }

    fn decode(&mut self, bytes: Vec<u8>) -> String {
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if std::mem::take(&mut self.first_line) && text.starts_with('\u{feff}') {
            text.remove(0);
        }
        text
    }

    fn fill(&mut self) -> Result<(), String> {
        let mut buffer = [0u8; 8192];
        loop {
            match self.inner.read(&mut buffer) {
                Ok(0) => {
                    self.ended = true;
                    return Ok(());
                }
                Ok(n) => {
                    self.pending.extend_from_slice(&buffer[..n]);
                    return Ok(());
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("the stream broke: {e}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gives its bytes a few at a time, as a network may.
    struct Trickle {
        bytes: Vec<u8>,
        at: usize,
        step: usize,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.step.min(buf.len()).min(self.bytes.len() - self.at);
            buf[..n].copy_from_slice(&self.bytes[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    fn all(text: &str, step: usize) -> Vec<Event> {
        let mut events = Events::new(Trickle { bytes: text.as_bytes().to_vec(), at: 0, step });
        let mut out = Vec::new();
        while let Some(event) = events.next_event().unwrap() {
            out.push(event);
        }
        out
    }

    fn event(name: &str, data: &str) -> Event {
        Event { name: name.into(), data: data.into() }
    }

    #[test]
    fn events_are_cut_whatever_the_pieces_they_arrive_in() {
        let text =
            "\u{feff}: a comment\nevent: delta\ndata: {\"text\": \"héllo 日本\"}\n\r\ndata: one\r\ndata:two\rid: 7\n\n";
        let expected = vec![event("delta", "{\"text\": \"héllo 日本\"}"), event("message", "one\ntwo")];
        // one byte at a time: lines, `\r\n` and multi-byte characters split between reads
        for step in [1, 2, 3, 5, 7, 4096] {
            assert_eq!(all(text, step), expected, "{step} byte(s) a read");
        }
    }

    #[test]
    fn an_event_without_data_is_skipped_and_one_cut_short_is_dropped() {
        assert_eq!(all("event: ping\n\ndata: kept\n\ndata: cut sh", 3), vec![event("message", "kept")]);
        assert_eq!(all("", 1), vec![]);
    }
}
