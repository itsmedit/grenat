//! Server-Sent Events, written: a response whose body is sent event by
//! event, each flushed as it is written, so that a browser shows a model's
//! answer while it is being written.
//!
//! An event's data may hold any text: each of its lines becomes a `data:`
//! line, so no text can end an event early or forge another one.

use std::io::Write;

/// The text of one event: `event: <name>` when it has one, its data.
pub fn event(name: Option<&str>, data: &str) -> String {
    let mut text = String::new();
    if let Some(name) = name {
        text.push_str(&format!("event: {name}\n"));
    }
    // `\r\n`, `\r` and `\n` all end a line for the reader
    for line in data.split("\r\n").flat_map(|l| l.split(['\r', '\n'])) {
        text.push_str("data: ");
        text.push_str(line);
        text.push('\n');
    }
    text.push('\n');
    text
}

/// Whether `name` can name an event: one line, not empty.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\r', '\n'])
}

/// A response being streamed: its head is written, its body goes out
/// event by event. Chunked over HTTP/1.1; over HTTP/1.0, the end of the
/// connection ends it.
pub struct EventStream {
    writer: Box<dyn Write + Send>,
    chunked: bool,
    finished: bool,
}

impl EventStream {
    /// Writes the head of the response (`text/event-stream`, never cached).
    pub(crate) fn start(
        mut writer: Box<dyn Write + Send>,
        status: u16,
        headers: &[(String, String)],
        http11: bool,
    ) -> std::io::Result<EventStream> {
        let mut head = format!("HTTP/1.{} {status} {}\r\n", u8::from(http11), reason(status));
        head.push_str("Content-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-cache\r\n");
        // proxies that buffer (nginx) pass the events on at once
        head.push_str("X-Accel-Buffering: no\r\n");
        for (name, value) in headers {
            let forbidden = ["content-type", "content-length", "transfer-encoding", "connection"];
            if forbidden.iter().any(|f| name.eq_ignore_ascii_case(f)) || !header_safe(name) || !header_safe(value) {
                continue;
            }
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(if http11 { "Transfer-Encoding: chunked\r\n" } else { "Connection: close\r\n" });
        head.push_str("\r\n");
        writer.write_all(head.as_bytes())?;
        writer.flush()?;
        Ok(EventStream { writer, chunked: http11, finished: false })
    }

    /// Sends an event, at once. An error means the client has gone.
    pub fn send(&mut self, name: Option<&str>, data: &str) -> std::io::Result<()> {
        self.write(event(name, data).as_bytes())
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.chunked {
            write!(self.writer, "{:x}\r\n", bytes.len())?;
            self.writer.write_all(bytes)?;
            self.writer.write_all(b"\r\n")?;
        } else {
            self.writer.write_all(bytes)?;
        }
        self.writer.flush()
    }

    /// Ends the response.
    pub fn finish(mut self) -> std::io::Result<()> {
        self.end()
    }

    fn end(&mut self) -> std::io::Result<()> {
        if std::mem::replace(&mut self.finished, true) {
            return Ok(());
        }
        if self.chunked {
            self.writer.write_all(b"0\r\n\r\n")?;
        }
        self.writer.flush()
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        let _ = self.end();
    }
}

/// A header name or value that cannot break the head of the response.
fn header_safe(text: &str) -> bool {
    !text.contains(['\r', '\n'])
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn data_lines_cannot_forge_events() {
        assert_eq!(event(None, "Hello"), "data: Hello\n\n");
        assert_eq!(event(Some("done"), ""), "event: done\ndata: \n\n");
        assert_eq!(
            event(None, "a\n\nevent: admin\r\ndata: x\rb"),
            "data: a\ndata: \ndata: event: admin\ndata: data: x\ndata: b\n\n"
        );
        assert!(valid_name("delta") && !valid_name("a\nb") && !valid_name(""));
    }

    /// A writer whose bytes the test reads.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_stream_is_chunked_over_http_1_1_and_closed_over_1_0() {
        let out = Shared::default();
        let headers = [("X-Trace".to_string(), "7".to_string()), ("Bad".to_string(), "a\r\nSet-Cookie: x".to_string())];
        let mut stream = EventStream::start(Box::new(out.clone()), 200, &headers, true).unwrap();
        stream.send(None, "é").unwrap();
        stream.finish().unwrap();
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert_eq!(
            text,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-cache\r\nX-Accel-Buffering: no\r\nX-Trace: 7\r\nTransfer-Encoding: chunked\r\n\r\n\
             a\r\ndata: é\n\n\r\n0\r\n\r\n"
        );

        let out = Shared::default();
        let mut stream = EventStream::start(Box::new(out.clone()), 200, &[], false).unwrap();
        stream.send(Some("end"), "x").unwrap();
        drop(stream);
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert!(text.starts_with("HTTP/1.0 200 OK\r\n") && text.contains("Connection: close\r\n"), "{text}");
        assert!(text.ends_with("\r\n\r\nevent: end\ndata: x\n\n"), "{text}");
    }
}
