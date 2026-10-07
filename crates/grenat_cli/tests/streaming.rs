//! `grenat serve` streams: a route answers as Server-Sent Events while the
//! model writes, each event reaching the client as soon as it is written.
//!
//! A local server plays the Messages API and holds the rest of its answer
//! until the client has read the first event: had anything been buffered on
//! the way, the client would wait for it forever.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// An event of the Messages API, as it is streamed.
fn event(name: &str, data: serde_json::Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

fn text(piece: &str) -> String {
    event(
        "content_block_delta",
        serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": piece}}),
    )
}

/// Plays the Messages API once: the start of the answer, then — once `go`
/// says so — its end.
fn model_server(go: mpsc::Receiver<()>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap();
            }
            if line.trim_end().is_empty() {
                break;
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["stream"], true);
        let start = [
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n".to_string(),
            event(
                "message_start",
                serde_json::json!({"type": "message_start", "message": {"model": "claude-haiku-4-5", "content": [], "usage": {"input_tokens": 9, "output_tokens": 1}}}),
            ),
            event(
                "content_block_start",
                serde_json::json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            ),
            text("Hel"),
        ];
        stream.write_all(start.concat().as_bytes()).unwrap();
        stream.flush().unwrap();
        go.recv_timeout(Duration::from_secs(20)).unwrap();
        let end = [
            text("lo, "),
            text("wörld"),
            event("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
            event(
                "message_delta",
                serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 4}}),
            ),
            event("message_stop", serde_json::json!({"type": "message_stop"})),
        ];
        stream.write_all(end.concat().as_bytes()).unwrap();
    });
    url
}

/// Reads until `bytes` holds `wanted`.
fn read_until(stream: &mut TcpStream, bytes: &mut Vec<u8>, wanted: &str) {
    let mut buffer = [0u8; 4096];
    while !String::from_utf8_lossy(bytes).contains(wanted) {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0, "the stream ended before {wanted:?}: {}", String::from_utf8_lossy(bytes));
        bytes.extend_from_slice(&buffer[..n]);
    }
}

/// The body of a chunked response.
fn dechunk(mut body: &[u8]) -> String {
    let mut out = Vec::new();
    loop {
        let line_end = body.windows(2).position(|w| w == b"\r\n").unwrap();
        let size = usize::from_str_radix(std::str::from_utf8(&body[..line_end]).unwrap(), 16).unwrap();
        if size == 0 {
            return String::from_utf8(out).unwrap();
        }
        out.extend_from_slice(&body[line_end + 2..line_end + 2 + size]);
        body = &body[line_end + 2 + size + 2..];
    }
}

#[test]
fn serve_streams_a_model_s_answer_as_it_is_written() {
    let (go, wait) = mpsc::channel();
    let model_url = model_server(wait);
    let src = format!(
        "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\", base_url: \"{model_url}\"
prompt reply(q: String) -> ~String using :fast
  user q
end
get \"/chat\" do |req|
  stream do |out|
    reply(req.params[\"q\"]) {{ |chunk| out << chunk }}
    out.event(\"done\", \"\")
  end
end
get \"/broken\" do |req|
  stream do |out|
    out << \"start\"
    raise ArgumentError, \"boom\"
  end
end
"
    );
    let dir = std::env::temp_dir().join(format!("grenat-cli-stream-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("chat.grn");
    std::fs::write(&path, src).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(["serve", "--listen", "127.0.0.1:0", path.to_str().unwrap()])
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("NO_COLOR", "1")
        .current_dir(&dir)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap()).read_line(&mut line).unwrap();
    let address = line.trim().strip_prefix("listening on http://").unwrap_or_else(|| panic!("{line}")).to_string();

    let mut client = TcpStream::connect(&address).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(client, "GET /chat?q=hi HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut bytes = Vec::new();
    // the first piece arrives while the model is still writing
    read_until(&mut client, &mut bytes, "data: Hel\n\n");
    go.send(()).unwrap();
    client.read_to_end(&mut bytes).unwrap();
    // a failure once the head is sent: an `error` event ends the stream
    let mut broken = TcpStream::connect(&address).unwrap();
    broken.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(broken, "GET /broken HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut failed = Vec::new();
    broken.read_to_end(&mut failed).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    let response = String::from_utf8_lossy(&bytes).into_owned();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let head = head.to_lowercase();
    assert!(head.contains("content-type: text/event-stream; charset=utf-8"), "{head}");
    assert!(head.contains("transfer-encoding: chunked"), "{head}");
    assert!(head.contains("cache-control: no-cache"), "{head}");
    assert_eq!(dechunk(body.as_bytes()), "data: Hel\n\ndata: lo, \n\ndata: wörld\n\nevent: done\ndata: \n\n");
    let failed = String::from_utf8(failed).unwrap();
    let (head, body) = failed.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(dechunk(body.as_bytes()), "data: start\n\nevent: error\ndata: error\n\n");
}

/// Clients that ask for an endless stream and never read it, more of them
/// than the server has workers: another request is answered all the same,
/// and each stalled stream ends (`StreamClosed`, logged).
#[test]
fn clients_that_stop_reading_hold_no_worker() {
    let src = "get \"/big\" do |req|
  stream do |out|
    chunk = \"x\" * 100000
    while true
      out << chunk
    end
  end
end
get \"/plain\" do |req|
  \"plain\"
end
";
    let dir = std::env::temp_dir().join(format!("grenat-cli-stall-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stall.grn");
    std::fs::write(&path, src).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(["serve", "--listen", "127.0.0.1:0", path.to_str().unwrap()])
        .env("GRENAT_LOG", "1")
        .env("NO_COLOR", "1")
        .current_dir(&dir)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut log = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    while !line.starts_with("listening on") {
        line.clear();
        assert!(log.read_line(&mut line).unwrap() > 0, "the server ended");
    }
    let address = line.trim().strip_prefix("listening on http://").unwrap_or_else(|| panic!("{line}")).to_string();
    let (lines, logged) = mpsc::channel();
    std::thread::spawn(move || log.lines().map_while(Result::ok).try_for_each(|l| lines.send(l)));

    let mut log = Vec::new();
    let wait_until = |log: &mut Vec<String>, done: &dyn Fn(&[String]) -> bool| {
        while !done(log) {
            match logged.recv_timeout(Duration::from_secs(30)) {
                Ok(line) => log.push(line),
                Err(_) => break,
            }
        }
    };
    let started = |log: &[String]| log.iter().filter(|l| l.contains("GET /big → 200")).count();
    let closed = |log: &[String]| log.iter().filter(|l| l.contains("GET /big: the client closed the stream")).count();

    // one connection at a time: tiny_http's connection pool can leave a
    // burst of connections queued behind threads that never free up
    // (each one holds a stalled client), which is not what this test is about
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut stalled = Vec::new();
    for n in 1..=workers + 2 {
        let mut client = TcpStream::connect(&address).unwrap();
        write!(client, "GET /big HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        stalled.push(client);
        wait_until(&mut log, &|log| started(log) >= n);
    }
    let mut plain = TcpStream::connect(&address).unwrap();
    plain.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(plain, "GET /plain HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    let read = plain.read_to_string(&mut answer);
    // each stalled stream ends, the clients still connected
    wait_until(&mut log, &|log| closed(log) >= workers + 2);
    child.kill().unwrap();
    child.wait().unwrap();
    drop(stalled);
    let _ = std::fs::remove_dir_all(&dir);
    read.unwrap_or_else(|e| panic!("/plain was not answered: {e}"));
    assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("plain"), "{answer}");
    assert_eq!(closed(&log), workers + 2, "{log:#?}");
}
