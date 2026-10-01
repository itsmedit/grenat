//! Bytes for a client, written by a thread of their own: whoever sends them
//! never waits on the network. A client that stops reading fills its
//! socket, then this outbox — and the next send fails, as for a client gone,
//! instead of holding the sender (a green worker) for as long as the client
//! likes.
//!
//! A client is stalled when the bytes waiting for it pass a limit, or when
//! one write has not ended for a while. Its writing thread stays blocked
//! until the client reads or closes its connection, but nothing waits on it.

use std::collections::VecDeque;
use std::io::{ErrorKind, Write};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// When a client is stalled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Bytes waiting to be written, at most (a single larger send is taken
    /// when nothing waits).
    pub pending: usize,
    /// How long a single write may last.
    pub write: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { pending: 1024 * 1024, write: Duration::from_secs(30) }
    }
}

/// What the sender and the writing thread share.
#[derive(Default)]
struct State {
    queue: VecDeque<Vec<u8>>,
    pending: usize,
    /// Since when the current write has lasted.
    writing_since: Option<Instant>,
    /// Why nothing more can be sent.
    failed: Option<ErrorKind>,
    /// No more bytes will come: the thread ends once the queue is written.
    closed: bool,
}

pub(crate) struct Outbox {
    shared: Arc<(Mutex<State>, Condvar)>,
    limits: Limits,
    thread: Option<JoinHandle<()>>,
}

impl Outbox {
    pub(crate) fn new(writer: Box<dyn Write + Send>, limits: Limits) -> std::io::Result<Outbox> {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let theirs = shared.clone();
        let thread =
            std::thread::Builder::new().name("grenat-outbox".into()).spawn(move || write_all(writer, &theirs))?;
        Ok(Outbox { shared, limits, thread: Some(thread) })
    }

    /// Queues `bytes`; an error when the client has gone or is stalled.
    pub(crate) fn send(&self, bytes: Vec<u8>) -> std::io::Result<()> {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(kind) = state.failed {
            return Err(kind.into());
        }
        let slow = state.writing_since.is_some_and(|since| since.elapsed() > self.limits.write);
        if slow || (state.pending > 0 && state.pending + bytes.len() > self.limits.pending) {
            state.failed = Some(ErrorKind::TimedOut);
            return Err(ErrorKind::TimedOut.into());
        }
        state.pending += bytes.len();
        state.queue.push_back(bytes);
        wake.notify_one();
        Ok(())
    }

    /// No more bytes: those queued are written, then the writer is dropped.
    pub(crate) fn close(&self) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
        wake.notify_one();
    }

    /// Closes, then waits until everything is written (or failed).
    pub(crate) fn wait(&mut self) -> std::io::Result<()> {
        self.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let (lock, _) = &*self.shared;
        match lock.lock().unwrap_or_else(PoisonError::into_inner).failed {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        }
    }
}

impl Drop for Outbox {
    fn drop(&mut self) {
        self.close();
    }
}

/// The writing thread: each piece written and flushed, in order.
fn write_all(mut writer: Box<dyn Write + Send>, shared: &(Mutex<State>, Condvar)) {
    let (lock, wake) = shared;
    loop {
        let bytes = {
            let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if state.failed.is_some() {
                    return;
                }
                if let Some(bytes) = state.queue.pop_front() {
                    state.writing_since = Some(Instant::now());
                    break bytes;
                }
                if state.closed {
                    return;
                }
                state = wake.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let written = writer.write_all(&bytes).and_then(|()| writer.flush());
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        state.writing_since = None;
        state.pending -= bytes.len();
        if let Err(e) = written {
            state.failed.get_or_insert(e.kind());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer that takes `allowed` bytes, then blocks until the test ends.
    struct Stalled {
        allowed: usize,
        written: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for Stalled {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.allowed == 0 {
                std::thread::sleep(Duration::from_secs(3600));
            }
            let n = buf.len().min(self.allowed);
            self.allowed -= n;
            self.written.lock().unwrap().extend_from_slice(&buf[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_client_that_stops_reading_never_holds_the_sender() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let writer = Stalled { allowed: 4, written: written.clone() };
        let limits = Limits { pending: 10, write: Duration::from_secs(3600) };
        let outbox = Outbox::new(Box::new(writer), limits).unwrap();
        let started = Instant::now();
        // the writer takes 4 bytes, then blocks: the 8 bytes wait for it
        outbox.send(b"abcdefgh".to_vec()).unwrap();
        while written.lock().unwrap().len() < 4 {
            std::thread::sleep(Duration::from_millis(5));
        }
        // more waits, up to the limit; then the client is stalled
        outbox.send(b"12".to_vec()).unwrap();
        assert_eq!(outbox.send(b"x".to_vec()).unwrap_err().kind(), ErrorKind::TimedOut);
        assert!(outbox.send(b"z".to_vec()).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(&*written.lock().unwrap(), b"abcd");
    }

    #[test]
    fn a_write_that_lasts_too_long_stalls_the_client() {
        let writer = Stalled { allowed: 0, written: Arc::default() };
        let limits = Limits { pending: 1 << 20, write: Duration::from_millis(50) };
        let outbox = Outbox::new(Box::new(writer), limits).unwrap();
        outbox.send(b"a".to_vec()).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(outbox.send(b"b".to_vec()).unwrap_err().kind(), ErrorKind::TimedOut);
    }

    /// A writer whose bytes the test reads, failing after `fail_after` writes.
    struct Counted {
        writes: usize,
        fail_after: usize,
        out: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for Counted {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.writes == self.fail_after {
                return Err(ErrorKind::BrokenPipe.into());
            }
            self.writes += 1;
            self.out.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn bytes_are_written_in_order_and_a_client_gone_is_said() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let counted = Counted { writes: 0, fail_after: usize::MAX, out: out.clone() };
        let mut outbox = Outbox::new(Box::new(counted), Limits::default()).unwrap();
        for piece in ["a", "b", "c"] {
            outbox.send(piece.as_bytes().to_vec()).unwrap();
        }
        outbox.wait().unwrap();
        assert_eq!(&*out.lock().unwrap(), b"abc");

        let counted = Counted { writes: 0, fail_after: 1, out: Arc::default() };
        let mut outbox = Outbox::new(Box::new(counted), Limits::default()).unwrap();
        outbox.send(b"a".to_vec()).unwrap();
        outbox.send(b"b".to_vec()).unwrap();
        assert_eq!(outbox.wait().unwrap_err().kind(), ErrorKind::BrokenPipe);
    }
}
