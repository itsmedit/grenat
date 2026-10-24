//! The file descriptors the server may take for its connections: it never
//! calls `accept` without one free for it.
//!
//! On macOS (as on the BSDs) `accept` takes the connection off the queue
//! before it allocates its descriptor, and when none is left (`EMFILE`) the
//! connection is dropped, its client reset; Linux leaves it queued. So the
//! server counts the sockets it holds — served, refused, handed to a stream
//! — and accepts only while that count is below its budget and a descriptor
//! is free at that moment. Otherwise it waits for one of its sockets to
//! close (or for a moment, when the program holds the rest), and the
//! connections wait in the system's queue.
//!
//! The budget is the process's limit (`RLIMIT_NOFILE`), less the
//! descriptors open when the server starts, less a reserve for what the
//! program opens later (files, databases, its own HTTP calls): an eighth of
//! the limit, between 8 and 128.

use std::net::{TcpListener, TcpStream};
use std::ops::Deref;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

/// How long a wait for room lasts before it looks again (a descriptor the
/// program closed, the server stopped): a socket closed wakes it at once.
const RECHECK: Duration = Duration::from_millis(10);

/// The sockets held, against the budget.
pub(crate) struct Descriptors {
    held: Mutex<usize>,
    closed: Condvar,
    budget: usize,
}

impl Descriptors {
    /// The budget of this process, as it is now.
    pub(crate) fn of_this_process() -> Arc<Descriptors> {
        Descriptors::with_budget(budget(limit(), open()))
    }

    pub(crate) fn with_budget(budget: usize) -> Arc<Descriptors> {
        Arc::new(Descriptors { held: Mutex::new(0), closed: Condvar::new(), budget })
    }

    /// Waits until a connection of `listener` may be accepted (`true`), or
    /// the server is `stopped` (`false`).
    pub(crate) fn wait_for_room(&self, listener: &TcpListener, stopped: &AtomicBool) -> bool {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if stopped.load(Ordering::Acquire) {
                return false;
            }
            if *held < self.budget && a_descriptor_is_free(listener) {
                return true;
            }
            held = self.closed.wait_timeout(held, RECHECK).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Counts `stream` until it is closed.
    pub(crate) fn hold(self: &Arc<Self>, stream: TcpStream) -> Socket {
        *self.held.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Socket { stream, _held: Held(self.clone()) }
    }

    #[cfg(test)]
    fn held(&self) -> usize {
        *self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A socket accepted, counted until it is closed — wherever it went.
pub(crate) struct Socket {
    // closed before it stops being counted: fields drop in order
    stream: TcpStream,
    _held: Held,
}

impl Deref for Socket {
    type Target = TcpStream;

    fn deref(&self) -> &TcpStream {
        &self.stream
    }
}

struct Held(Arc<Descriptors>);

impl Drop for Held {
    fn drop(&mut self) {
        *self.0.held.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0.closed.notify_all();
    }
}

/// What the server may hold, `open` descriptors already taken out of
/// `limit` (`None`: no limit).
fn budget(limit: Option<usize>, open: usize) -> usize {
    let Some(limit) = limit else { return usize::MAX };
    let reserve = (limit / 8).clamp(8, 128);
    limit.saturating_sub(open + reserve).max(1)
}

/// `RLIMIT_NOFILE`, the current (soft) limit.
fn limit() -> Option<usize> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes to the struct it is given, nothing else.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 || limit.rlim_cur == libc::RLIM_INFINITY {
        return None;
    }
    usize::try_from(limit.rlim_cur).ok()
}

/// The descriptors open (`/dev/fd`, Linux's and macOS's alike), less the
/// one listing them.
fn open() -> usize {
    std::fs::read_dir("/dev/fd").map_or(0, |entries| entries.count().saturating_sub(1))
}

/// Whether a descriptor is free now: one more is taken, then given back.
fn a_descriptor_is_free(listener: &TcpListener) -> bool {
    // SAFETY: duplicating a descriptor the listener owns, closed at once.
    let duplicate = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return false;
    }
    // SAFETY: the duplicate is ours alone.
    unsafe { libc::close(duplicate) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn pair(listener: &TcpListener) -> TcpStream {
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        listener.accept().unwrap().0
    }

    #[test]
    fn the_budget_keeps_a_reserve_for_the_program() {
        assert_eq!(budget(Some(64), 10), 46);
        assert_eq!(budget(Some(256), 10), 214);
        assert_eq!(budget(Some(1024), 10), 886);
        assert_eq!(budget(Some(100_000), 10), 99_862);
        // never none at all, nor a limit of nothing
        assert_eq!(budget(Some(16), 20), 1);
        assert_eq!(budget(None, 10), usize::MAX);
    }

    #[test]
    fn the_limit_and_the_open_descriptors_are_read() {
        let limit = limit().unwrap_or(usize::MAX);
        let open = open();
        // standard input, output and error at least
        assert!(open >= 3 && open < limit, "{open} open of {limit}");
    }

    #[test]
    fn a_socket_is_counted_until_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let descriptors = Descriptors::with_budget(2);
        let first = Arc::new(descriptors.hold(pair(&listener)));
        let shared = first.clone();
        let second = descriptors.hold(pair(&listener));
        assert_eq!(descriptors.held(), 2);
        drop(first);
        assert_eq!(descriptors.held(), 2, "a stream still holds it");
        drop(shared);
        drop(second);
        assert_eq!(descriptors.held(), 0);
    }

    #[test]
    fn a_full_budget_waits_for_a_socket_to_close() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let descriptors = Descriptors::with_budget(1);
        let stopped = AtomicBool::new(false);
        assert!(descriptors.wait_for_room(&listener, &stopped));
        let held = descriptors.hold(pair(&listener));
        let closing = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(held);
        });
        let waited = Instant::now();
        assert!(descriptors.wait_for_room(&listener, &stopped));
        assert!(waited.elapsed() >= Duration::from_millis(150), "{:?}", waited.elapsed());
        closing.join().unwrap();
    }

    #[test]
    fn a_stopped_server_waits_no_more() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let descriptors = Descriptors::with_budget(1);
        let _held = descriptors.hold(pair(&listener));
        assert!(!descriptors.wait_for_room(&listener, &AtomicBool::new(true)));
    }

    #[test]
    fn a_descriptor_free_is_seen_and_given_back() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        // given back each time: never running out, whatever the limit
        assert!((0..100_000).all(|_| a_descriptor_is_free(&listener)));
    }
}
