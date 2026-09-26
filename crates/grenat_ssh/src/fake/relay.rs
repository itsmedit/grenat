//! A TCP relay to a local port that a test can cut, both ways at once: a
//! connection that breaks while a command runs.

use std::io::copy;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Relay {
    pub port: u16,
    /// Both ends of every relayed connection.
    streams: Arc<Mutex<Vec<TcpStream>>>,
}

impl Relay {
    /// A relay to `127.0.0.1:target`.
    pub fn start(target: u16) -> Relay {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let streams = Arc::new(Mutex::new(Vec::new()));
        let kept = streams.clone();
        std::thread::spawn(move || {
            for client in listener.incoming() {
                let (Ok(client), Ok(server)) = (client, TcpStream::connect(("127.0.0.1", target))) else { continue };
                kept.lock().unwrap().extend([client.try_clone().unwrap(), server.try_clone().unwrap()]);
                pipe(client.try_clone().unwrap(), server.try_clone().unwrap());
                pipe(server, client);
            }
        });
        Relay { port, streams }
    }

    /// Cuts every relayed connection after `delay`, without waiting.
    pub fn cut_after(&self, delay: Duration) {
        let streams = self.streams.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            for stream in streams.lock().unwrap().iter() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        });
    }
}

/// Copies what `from` receives to `to`, until either closes.
fn pipe(mut from: TcpStream, mut to: TcpStream) {
    std::thread::spawn(move || {
        let _ = copy(&mut from, &mut to);
        let _ = to.shutdown(Shutdown::Write);
    });
}
