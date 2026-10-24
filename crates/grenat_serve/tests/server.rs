//! The HTTP server against raw sockets: connections kept alive, requests
//! refused before the program sees them, and clients that cannot hold the
//! server — idle, slow, stalled, too many, or all at once.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use grenat_serve::{Server, Settings, Sizes, Timeouts};

/// A server answering each request with its method, path, query and body;
/// `/stream` streams events until its client is stalled, then says when.
fn start(settings: Settings) -> (String, std::sync::mpsc::Receiver<Duration>) {
    let server = Server::bind_with("127.0.0.1:0", settings).unwrap();
    let address = server.address();
    let (stalled, told) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(incoming) = server.next() {
            if incoming.path == "/stream" {
                let stalled = stalled.clone();
                std::thread::spawn(move || {
                    let mut events = incoming.respond_events(200, &[]).unwrap();
                    let started = Instant::now();
                    while events.send(None, &"x".repeat(100_000)).is_ok() {}
                    let _ = stalled.send(started.elapsed());
                });
                continue;
            }
            let text = format!(
                "{} {} {} {}",
                incoming.method,
                incoming.path,
                incoming.query,
                String::from_utf8_lossy(&incoming.body)
            );
            incoming.respond(200, "text/plain", &[("X-Seen".into(), "1".into())], text);
        }
    });
    (address, told)
}

fn connect(address: &str) -> TcpStream {
    let client = TcpStream::connect(address).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    client
}

/// Reads one response whose length is given, or until the connection ends.
fn response(client: &mut TcpStream) -> String {
    read_response(client, true)
}

/// Reads the head of a response to `HEAD`: its length is given, but no body follows.
fn head_response(client: &mut TcpStream) -> String {
    read_response(client, false)
}

/// Bytes read past the response asked for — the start of the next one, when
/// pipelined responses arrive in one read — kept for the next call, by client.
static PENDING: std::sync::Mutex<Vec<(std::net::SocketAddr, Vec<u8>)>> = std::sync::Mutex::new(Vec::new());

fn read_response(client: &mut TcpStream, with_body: bool) -> String {
    let key = client.local_addr().unwrap();
    let mut bytes = {
        let mut pending = PENDING.lock().unwrap();
        let index = pending.iter().position(|(k, _)| *k == key);
        index.map(|i| pending.remove(i).1).unwrap_or_default()
    };
    let mut buffer = [0; 4096];
    loop {
        if let Some(end) = message_end(&bytes, with_body) {
            let rest = bytes.split_off(end);
            if !rest.is_empty() {
                PENDING.lock().unwrap().push((key, rest));
            }
            return String::from_utf8_lossy(&bytes).into_owned();
        }
        match client.read(&mut buffer) {
            Ok(0) | Err(_) => return String::from_utf8_lossy(&bytes).into_owned(),
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
    }
}

/// Where the first response in `bytes` ends, once it is whole: after its head
/// (a response to `HEAD`), or after the body its `Content-Length` gives.
fn message_end(bytes: &[u8], with_body: bool) -> Option<usize> {
    let head = bytes.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    if !with_body {
        return Some(head);
    }
    let text = String::from_utf8_lossy(&bytes[..head]);
    let length: usize = text.lines().find_map(|l| l.strip_prefix("Content-Length: "))?.trim().parse().ok()?;
    (bytes.len() >= head + length).then_some(head + length)
}

fn closed(client: &mut TcpStream) -> bool {
    matches!(client.read(&mut [0; 1]), Ok(0) | Err(_))
}

#[test]
fn requests_on_a_kept_alive_connection_are_answered_in_order() {
    let (address, _) = start(Settings::default());
    let mut client = connect(&address);
    // two requests at once, then a third: each answered, in order
    write!(client, "POST /a?x=1 HTTP/1.1\r\nHost: h\r\nContent-Length: 2\r\n\r\nhiGET /b HTTP/1.1\r\nHost: h\r\n\r\n")
        .unwrap();
    let first = response(&mut client);
    assert!(first.starts_with("HTTP/1.1 200 OK\r\n") && first.ends_with("POST /a x=1 hi"), "{first}");
    assert!(first.contains("X-Seen: 1\r\n") && first.contains("Date: "), "{first}");
    assert!(response(&mut client).ends_with("GET /b  "));
    write!(client, "HEAD /c HTTP/1.1\r\nHost: h\r\n\r\n").unwrap();
    let head = head_response(&mut client);
    assert!(head.contains("Content-Length: 9\r\n") && head.ends_with("\r\n\r\n"), "{head}");
    write!(client, "POST /d HTTP/1.1\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\nok\r\n0\r\n\r\n")
        .unwrap();
    // no body after the head of `HEAD`: the next response starts at once
    let last = response(&mut client);
    assert!(last.starts_with("HTTP/1.1 200 OK\r\n"), "{last}");
    assert!(last.contains("Connection: close\r\n") && last.ends_with("POST /d  ok"), "{last}");
    assert!(closed(&mut client));

    // HTTP/1.0 closes, unless kept alive
    let mut old = connect(&address);
    write!(old, "GET /e HTTP/1.0\r\n\r\n").unwrap();
    assert!(response(&mut old).starts_with("HTTP/1.0 200 OK\r\n"));
    assert!(closed(&mut old));
}

#[test]
fn a_client_waiting_for_100_continue_gets_it() {
    let (address, _) = start(Settings::default());
    let mut client = connect(&address);
    write!(client, "PUT /f HTTP/1.1\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n").unwrap();
    let mut interim = [0; 25];
    client.read_exact(&mut interim).unwrap();
    assert_eq!(&interim, b"HTTP/1.1 100 Continue\r\n\r\n");
    client.write_all(b"body").unwrap();
    assert!(response(&mut client).ends_with("PUT /f  body"));
}

#[test]
fn malformed_and_oversized_requests_are_refused_and_closed() {
    let sizes = Sizes { body: 8, ..Sizes::default() };
    let (address, _) = start(Settings { sizes, ..Settings::default() });
    for (request, status) in [
        ("GET / HTTP/1.1\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n", "400"),
        ("POST / HTTP/1.1\r\nContent-Length: 9\r\n\r\n", "413"),
        ("GET / HTTP/3\r\n\r\n", "505"),
        ("nonsense\r\n\r\n", "400"),
    ] {
        let mut client = connect(&address);
        client.write_all(request.as_bytes()).unwrap();
        let answer = response(&mut client);
        assert!(answer.starts_with(&format!("HTTP/1.1 {status} ")), "{request:?}: {answer}");
        assert!(closed(&mut client));
    }
}

#[test]
fn idle_and_slow_clients_are_let_go() {
    let short = Duration::from_millis(300);
    let timeouts = Timeouts { idle: short, request: short, write: short };
    let (address, _) = start(Settings { timeouts, ..Settings::default() });
    // connected, never asking
    let mut idle = connect(&address);
    let started = Instant::now();
    assert!(closed(&mut idle));
    assert!(started.elapsed() < Duration::from_secs(5));
    // a head sent a byte at a time, each in time, the whole too slowly
    let mut slow = connect(&address);
    let started = Instant::now();
    for byte in b"GET / HTTP/1.1\r\nHost: slow-client\r\n".iter().cycle().take(64) {
        if slow.write_all(&[*byte]).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(response(&mut slow).starts_with("HTTP/1.1 408 "));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn connections_beyond_the_limit_are_answered_503() {
    let (address, _) = start(Settings { connections: 2, ..Settings::default() });
    let held: Vec<TcpStream> = (0..2).map(|_| connect(&address)).collect();
    std::thread::sleep(Duration::from_millis(200));
    let mut third = connect(&address);
    assert!(response(&mut third).starts_with("HTTP/1.1 503 "));
    drop(held);
    // their room given back
    let started = Instant::now();
    loop {
        let mut client = connect(&address);
        write!(client, "GET /again HTTP/1.1\r\n\r\n").unwrap();
        if response(&mut client).ends_with("GET /again  ") {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "the room was never given back");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_burst_of_idle_and_stalled_clients_keeps_no_one_waiting() {
    let write = Duration::from_millis(500);
    let timeouts = Timeouts { write, ..Timeouts::default() };
    let (address, stalled) = start(Settings { timeouts, ..Settings::default() });
    // connected all at once: some never ask, some stream and never read
    let idle: Vec<TcpStream> = (0..100).map(|_| connect(&address)).collect();
    let mut streams: Vec<TcpStream> = (0..20).map(|_| connect(&address)).collect();
    for client in &mut streams {
        write!(client, "GET /stream HTTP/1.1\r\nHost: h\r\n\r\n").unwrap();
    }
    let started = Instant::now();
    let mut client = connect(&address);
    write!(client, "GET /now HTTP/1.1\r\n\r\n").unwrap();
    assert!(response(&mut client).ends_with("GET /now  "));
    assert!(started.elapsed() < Duration::from_secs(3), "answered after {:?}", started.elapsed());
    // each stream is told its client stalled, within its write timeout
    for _ in 0..20 {
        let after = stalled.recv_timeout(Duration::from_secs(20)).expect("a stalled stream never ended");
        assert!(after < Duration::from_secs(10), "{after:?}");
    }
    // and its connection ends, once its client reads what it was sent
    for mut client in streams {
        client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).unwrap();
        assert!(String::from_utf8_lossy(&bytes).starts_with("HTTP/1.1 200 OK\r\n"));
    }
    drop(idle);
}

#[test]
fn a_dropped_server_frees_its_port() {
    let server = Server::bind("127.0.0.1:0").unwrap();
    let address = server.address();
    drop(server);
    let started = Instant::now();
    while TcpStream::connect(&address).is_ok() {
        assert!(started.elapsed() < Duration::from_secs(5), "still listening");
        std::thread::sleep(Duration::from_millis(20));
    }
}
