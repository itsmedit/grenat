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

/// A program with an endless stream (`/big`) and a plain route (`/plain`).
const STALL: &str = "get \"/big\" do |req|
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

/// `grenat serve` of [`STALL`], logging, with at most `descriptors` open
/// files if given: the server, its address, its log lines as they come.
fn serve_stall(name: &str, descriptors: Option<u32>) -> (std::process::Child, String, mpsc::Receiver<String>) {
    let dir = std::env::temp_dir().join(format!("grenat-cli-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stall.grn");
    std::fs::write(&path, STALL).unwrap();
    let grenat = env!("CARGO_BIN_EXE_grenat");
    let mut command = match descriptors {
        Some(n) => {
            let mut sh = Command::new("sh");
            sh.args(["-c", &format!("ulimit -n {n} && exec \"$0\" \"$@\""), grenat]);
            sh
        }
        None => Command::new(grenat),
    };
    let mut child = command
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
    (child, address, logged)
}

/// `GET /plain`, answered within `within` or an error.
fn plain(address: &str, within: Duration) -> std::io::Result<String> {
    let mut plain = TcpStream::connect(address)?;
    plain.set_read_timeout(Some(within))?;
    write!(plain, "GET /plain HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")?;
    let mut answer = String::new();
    plain.read_to_string(&mut answer)?;
    Ok(answer)
}

/// Clients that ask for an endless stream and never read it, a burst of
/// them all at once, more than the server has threads of any kind: another
/// request is answered all the same, and each stalled stream starts, then
/// ends (`StreamClosed`, logged).
#[test]
fn clients_that_stop_reading_hold_no_worker() {
    let (mut child, address, logged) = serve_stall("stall", None);
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

    let burst = 4 * std::thread::available_parallelism().map_or(4, |n| n.get()) + 2;
    // all connected first, so that the server finds them all waiting at once
    let mut stalled: Vec<TcpStream> = (0..burst).map(|_| TcpStream::connect(&address).unwrap()).collect();
    for client in &mut stalled {
        write!(client, "GET /big HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    }
    let asked = std::time::Instant::now();
    let answer = plain(&address, Duration::from_secs(10));
    let answered_in = asked.elapsed();
    wait_until(&mut log, &|log| started(log) >= burst);
    let all_started_in = asked.elapsed();
    // each stalled stream ends, the clients still connected
    wait_until(&mut log, &|log| closed(log) >= burst);
    child.kill().unwrap();
    child.wait().unwrap();
    drop(stalled);
    let answer = answer.unwrap_or_else(|e| panic!("/plain was not answered: {e}"));
    assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("plain"), "{answer}");
    assert!(answered_in < Duration::from_secs(5), "/plain answered after {answered_in:?}");
    assert!(all_started_in < Duration::from_secs(5), "the streams started within {all_started_in:?}");
    assert_eq!(started(&log), burst, "{log:#?}");
    assert_eq!(closed(&log), burst, "{log:#?}");
}

/// A burst of clients beyond the server's file descriptors: those it
/// cannot take wait, and once the burst has gone, the server serves again.
#[test]
fn a_burst_beyond_the_descriptors_leaves_the_server_serving() {
    let (mut child, address, log) = serve_stall("descriptors", Some(64));
    // a connection the system refuses is part of the burst all the same
    let mut stalled: Vec<TcpStream> = (0..100).filter_map(|_| TcpStream::connect(&address).ok()).collect();
    for client in &mut stalled {
        let _ = write!(client, "GET /big HTTP/1.1\r\nHost: x\r\n\r\n");
    }
    std::thread::sleep(Duration::from_millis(500));
    drop(stalled);
    let answer = plain(&address, Duration::from_secs(10));
    child.kill().unwrap();
    child.wait().unwrap();
    let log: Vec<String> = log.try_iter().collect();
    let answer = answer.unwrap_or_else(|e| panic!("/plain was not answered: {e}\n{log:#?}"));
    assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("plain"), "{answer:?}\n{log:#?}");
}
