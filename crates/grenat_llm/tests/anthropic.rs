//! Messages API HTTP client, tested against a local server that plays the API.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grenat_llm::{Anthropic, ModelConfig, Provider, Request, ToolSpec};
use serde_json::{Value as Json, json};

/// Request received by the fake server.
struct Received {
    path: String,
    headers: HashMap<String, String>,
    body: Json,
}

/// Fake server reply: status, extra headers, body.
type Reply = (u16, Vec<(&'static str, &'static str)>, Json);

/// Starts a server that answers with the `replies`, in order.
fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Received>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();
    std::thread::spawn(move || {
        for (status, headers, body) in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut request_headers = HashMap::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                request_headers.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
            let length: usize = request_headers.get("content-length").map_or(0, |l| l.parse().unwrap());
            let mut raw = vec![0; length];
            reader.read_exact(&mut raw).unwrap();
            log.lock().unwrap().push(Received {
                path: request_line.split_whitespace().nth(1).unwrap().to_string(),
                headers: request_headers,
                body: serde_json::from_slice(&raw).unwrap(),
            });
            let payload = body.to_string();
            let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{extra}\r\n{payload}",
                payload.len()
            );
            let mut stream = stream;
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (url, received)
}

fn message(text: &str) -> Json {
    json!({
        "model": "claude-opus-5",
        "stop_reason": "end_turn",
        "content": [{"type": "text", "text": text}],
        "usage": {"input_tokens": 12, "output_tokens": 3}
    })
}

fn error(kind: &str, text: &str) -> Json {
    json!({"type": "error", "error": {"type": kind, "message": text}})
}

fn request(model: &ModelConfig) -> Request<'_> {
    Request {
        model,
        system: Some("be brief".into()),
        messages: vec![json!({"role": "user", "content": "Hello"})],
        tools: vec![ToolSpec {
            name: "read".into(),
            description: "Reads".into(),
            input_schema: json!({"type": "object"}),
        }],
        output_schema: None,
    }
}

fn client(url: &str) -> Anthropic {
    Anthropic::new("sk-test", url).with_retry_delay(Duration::from_millis(5))
}

#[test]
fn sends_a_well_formed_request_and_parses_the_reply() {
    let (url, received) = serve(vec![(200, vec![], message("Hi"))]);
    let model = ModelConfig::new("anthropic", "claude-opus-5");
    let response = client(&url).complete(&request(&model)).unwrap();
    assert_eq!(response.text(), "Hi");
    assert_eq!(response.usage.input_tokens, 12);

    let received = received.lock().unwrap();
    let r = &received[0];
    assert_eq!(r.path, "/v1/messages");
    assert_eq!(r.headers["x-api-key"], "sk-test");
    assert_eq!(r.headers["anthropic-version"], "2023-06-01");
    // server-side fallback: on by default for claude-opus-5, with its beta header
    assert_eq!(r.headers["anthropic-beta"], "server-side-fallback-2026-07-01");
    assert_eq!(r.body["fallbacks"], "default");
    assert_eq!(r.body["system"], "be brief");
    assert_eq!(r.body["tools"][0]["strict"], true);
}

#[test]
fn no_beta_header_without_fallbacks() {
    let (url, received) = serve(vec![(200, vec![], message("ok"))]);
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    client(&url).complete(&request(&model)).unwrap();
    let received = received.lock().unwrap();
    assert!(!received[0].headers.contains_key("anthropic-beta"));
    assert!(received[0].body.get("fallbacks").is_none());
}

#[test]
fn retries_rate_limits_and_overload() {
    let (url, received) = serve(vec![
        (429, vec![("retry-after", "0")], error("rate_limit_error", "too fast")),
        (529, vec![], error("overloaded_error", "overloaded")),
        (200, vec![], message("finally")),
    ]);
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    assert_eq!(client(&url).complete(&request(&model)).unwrap().text(), "finally");
    assert_eq!(received.lock().unwrap().len(), 3);
}

#[test]
fn client_errors_are_not_retried() {
    let (url, received) = serve(vec![(400, vec![], error("invalid_request_error", "invalid schema"))]);
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let e = client(&url).complete(&request(&model)).unwrap_err();
    assert_eq!(e.message, "HTTP 400: invalid schema");
    assert_eq!(received.lock().unwrap().len(), 1);
}

#[test]
fn gives_up_after_the_last_attempt() {
    let overloaded = || (529, vec![], error("overloaded_error", "overloaded"));
    let (url, received) = serve(vec![overloaded(), overloaded(), overloaded(), overloaded()]);
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let e = client(&url).complete(&request(&model)).unwrap_err();
    assert_eq!(e.message, "HTTP 529: overloaded");
    assert_eq!(received.lock().unwrap().len(), 4);
}

#[test]
fn connection_failures_are_reported() {
    // closed port: nobody is listening
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let e = client(&format!("http://127.0.0.1:{port}")).complete(&request(&model)).unwrap_err();
    assert!(e.message.starts_with("connection failed"), "{}", e.message);
}
