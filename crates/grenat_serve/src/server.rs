//! An HTTP/1.x server: requests in, one response each — whole, or a stream
//! of events.
//!
//! Each connection is served by a thread of its own (see
//! [`crate::connection`]), as many at once as [`Settings::connections`]
//! says; a connection beyond them is answered `503` at once. The program
//! takes the requests one by one ([`Server::next`]), from every connection
//! at once: no client, however slow, keeps another one waiting.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use crate::EventStream;
use crate::connection::{self, Exchange, Reply, Timeouts};
use crate::request::Sizes;

/// What the server allows its clients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub timeouts: Timeouts,
    pub sizes: Sizes,
    /// Connections served at once (streams handed over no longer count).
    pub connections: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { timeouts: Timeouts::default(), sizes: Sizes::default(), connections: 512 }
    }
}

/// A request received, to answer with [`Incoming::respond`].
pub struct Incoming {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    http11: bool,
    socket: TcpStream,
    reply: mpsc::Sender<Reply>,
    /// When a stream's client is stalled.
    stalled: crate::outbox::Limits,
}

impl Incoming {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Answers whole; the connection's thread writes it, never the caller.
    pub fn respond(self, status: u16, content_type: &str, headers: &[(String, String)], body: String) {
        let reply = Reply::Whole { status, content_type: content_type.to_string(), headers: headers.to_vec(), body };
        let _ = self.reply.send(reply);
    }

    /// Starts a response of Server-Sent Events: its head is sent now, its
    /// events as they are written. The stream ends the connection.
    pub fn respond_events(self, status: u16, headers: &[(String, String)]) -> std::io::Result<EventStream> {
        let events = EventStream::start(Box::new(self.socket), status, headers, self.http11, self.stalled)?;
        let _ = self.reply.send(Reply::Streamed);
        Ok(events)
    }
}

pub struct Server {
    address: String,
    local: SocketAddr,
    settings: Settings,
    requests: Mutex<mpsc::Receiver<Exchange>>,
    /// Set once dropped: the accepting thread ends, the port is free.
    stopped: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // wakes the accepting thread
        let _ = TcpStream::connect_timeout(&self.local, Duration::from_secs(1));
    }
}

impl Server {
    /// Listens on `address` (`127.0.0.1:0` for any free port).
    pub fn bind(address: &str) -> Result<Server, String> {
        Server::bind_with(address, Settings::default())
    }

    pub fn bind_with(address: &str, settings: Settings) -> Result<Server, String> {
        let listener = TcpListener::bind(address).map_err(|e| format!("cannot listen on {address}: {e}"))?;
        let local = listener.local_addr().map_err(|e| format!("cannot listen on {address}: {e}"))?;
        let (program, requests) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::Builder::new()
            .name("grenat-accept".into())
            .spawn(move || accept(&listener, settings, &program, &stop))
            .map_err(|e| format!("cannot listen on {address}: {e}"))?;
        Ok(Server { address: local.to_string(), local, settings, requests: Mutex::new(requests), stopped })
    }

    /// Where it listens.
    pub fn address(&self) -> String {
        self.address.clone()
    }

    /// The next request (blocks).
    pub fn next(&self) -> Result<Incoming, String> {
        let requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
        let Exchange { request, socket, reply } = requests.recv().map_err(|_| "the server stopped".to_string())?;
        let (path, query) = match request.target.split_once('?') {
            Some((path, query)) => (path.to_string(), query.to_string()),
            None => (request.target.clone(), String::new()),
        };
        Ok(Incoming {
            method: request.method,
            path,
            query,
            headers: request.headers,
            body: request.body,
            http11: request.http11,
            socket,
            reply,
            stalled: crate::outbox::Limits { write: self.settings.timeouts.write, ..Default::default() },
        })
    }
}

/// Accepts connections, each served on a thread of its own while there is
/// room, until `stopped`. A failure to accept (out of descriptors, a
/// connection reset while queued) is waited out, never the end of the
/// server: the connections served meanwhile free what they hold.
fn accept(listener: &TcpListener, settings: Settings, program: &mpsc::Sender<Exchange>, stopped: &AtomicBool) {
    let live = Arc::new(AtomicUsize::new(0));
    for socket in listener.incoming() {
        if stopped.load(Ordering::Acquire) {
            return;
        }
        let mut socket = match socket {
            Ok(socket) => socket,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        if live.fetch_add(1, Ordering::AcqRel) >= settings.connections {
            live.fetch_sub(1, Ordering::AcqRel);
            let _ = socket.set_write_timeout(Some(Duration::from_secs(1)));
            connection::refuse(&mut socket, 503);
            continue;
        }
        let slot = Slot(live.clone());
        let program = program.clone();
        let spawned =
            std::thread::Builder::new().name("grenat-connection".into()).stack_size(256 * 1024).spawn(move || {
                let _slot = slot;
                connection::serve(socket, settings.timeouts, settings.sizes, &program);
            });
        if spawned.is_err() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A connection counted while it is served.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
