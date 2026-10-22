//! One client's connection, on a thread of its own: its requests read one
//! after the other (kept alive between them), each handed to the program,
//! its answer written back — or the socket handed to an event stream.
//!
//! No client holds the thread for as long as it likes: it has a while to
//! start a request, a while to send its head, a while for each read of its
//! body, and each write to it must end within a while too.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::request::{self, Failure, Request, Sizes};
use crate::response::{self, Answering};

/// How long a client may take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timeouts {
    /// To start a request, on a new or a kept-alive connection.
    pub idle: Duration,
    /// To send the head of a request, once started; and each read of its body.
    pub request: Duration,
    /// Each write to the client: a whole response, a piece of a stream.
    pub write: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        let thirty = Duration::from_secs(30);
        Timeouts { idle: thirty, request: thirty, write: thirty }
    }
}

/// A request for the program, and where its answer goes.
pub(crate) struct Exchange {
    pub request: Request,
    /// The client's socket, for a stream to write to.
    pub socket: TcpStream,
    pub reply: mpsc::Sender<Reply>,
}

/// What the program answers.
pub(crate) enum Reply {
    Whole {
        status: u16,
        content_type: String,
        headers: Vec<(String, String)>,
        body: String,
    },
    /// The socket was handed to an event stream, which ends the connection.
    Streamed,
}

/// Serves `socket` until the client closes it, stalls, or a stream takes
/// it. Requests go to `program`; none is read while one is being answered.
pub(crate) fn serve(socket: TcpStream, timeouts: Timeouts, sizes: Sizes, program: &mpsc::Sender<Exchange>) {
    let _ = socket.set_nodelay(true);
    if socket.set_write_timeout(Some(timeouts.write)).is_err() {
        return;
    }
    let Ok(reading) = socket.try_clone() else { return };
    let mut reader = BufReader::new(Deadline { socket: reading, until: None });
    let mut writer = socket;
    loop {
        // the first byte of the next request, within `idle`
        reader.get_mut().until = Some(Instant::now() + timeouts.idle);
        if !matches!(reader.fill_buf(), Ok(bytes) if !bytes.is_empty()) {
            return;
        }
        reader.get_mut().until = Some(Instant::now() + timeouts.request);
        let head = request::read_head(&mut reader, &mut writer, sizes);
        // the body: each read within `request`, however long the whole
        reader.get_mut().until = None;
        reader.get_ref().socket.set_read_timeout(Some(timeouts.request)).ok();
        let read = head.and_then(|(mut request, framing)| {
            request.body = request::read_body(&mut reader, &framing, sizes)?;
            Ok(request)
        });
        let request = match read {
            Ok(request) => request,
            Err(Failure::Gone) => return,
            Err(Failure::Refused(status)) => return refuse(&mut writer, status),
        };
        let to = Answering {
            http11: request.http11,
            keep_alive: request.keep_alive(),
            head_only: request.method.eq_ignore_ascii_case("HEAD"),
        };
        let Ok(socket) = writer.try_clone() else { return };
        let (reply, answer) = mpsc::channel();
        if program.send(Exchange { request, socket, reply }).is_err() {
            return refuse(&mut writer, 503);
        }
        // an exchange dropped unanswered: the connection is closed
        let Ok(Reply::Whole { status, content_type, headers, body }) = answer.recv() else { return };
        let bytes = response::whole(status, &content_type, &headers, body.as_bytes(), to, now());
        if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() || !to.keep_alive {
            return;
        }
    }
}

/// Answers a request refused before the program saw it, and closes.
pub(crate) fn refuse(writer: &mut TcpStream, status: u16) {
    let to = Answering { http11: true, keep_alive: false, head_only: false };
    let body = crate::status::reason(status);
    let _ = writer.write_all(&response::whole(status, "text/plain; charset=utf-8", &[], body.as_bytes(), to, now()));
    let _ = writer.shutdown(std::net::Shutdown::Write);
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The socket, read until a deadline: each read waits only for what is
/// left of it (a client sending a byte now and then gains nothing).
struct Deadline {
    socket: TcpStream,
    until: Option<Instant>,
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(until) = self.until {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ErrorKind::TimedOut.into());
            }
            self.socket.set_read_timeout(Some(left))?;
        }
        self.socket.read(buf)
    }
}
