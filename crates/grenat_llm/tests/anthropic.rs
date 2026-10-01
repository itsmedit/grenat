//! Messages API HTTP client, tested against a local server that plays the API.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grenat_llm::{Anthropic, CacheTtl, Caching, ModelConfig, Provider, Request, ToolSpec};
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
    let url_for_replies = url.clone();
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
                // a GET has no body
                body: serde_json::from_slice(&raw).unwrap_or(Json::Null),
            });
            // a string is sent as it is (JSON Lines); `BASE` is this server
            let payload = match &body {
                Json::String(text) => text.clone(),
                other => other.to_string(),
            }
            .replace("BASE", &url_for_replies);
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
            strict: true,
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
    assert_eq!(r.body["system"][0]["text"], "be brief");
    assert_eq!(r.body["tools"][0]["strict"], true);
}

#[test]
fn the_attempts_a_fallback_followed_are_read_from_the_iterations() {
    let mut reply = message("Hi");
    reply["model"] = json!("claude-opus-4-8");
    reply["usage"] = json!({
        "input_tokens": 412, "output_tokens": 264, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0,
        "iterations": [
            // declined before writing: billed in some categories only, not named here
            {"type": "message", "model": "claude-fable-5", "input_tokens": 535, "output_tokens": 0},
            // declined after writing: billed
            {"type": "message", "model": "claude-opus-5", "input_tokens": 400, "output_tokens": 2000, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 0},
            {"type": "fallback_message", "model": "claude-opus-4-8", "input_tokens": 412, "output_tokens": 264}
        ]
    });
    // without a fallback, iterations are not attempts besides the answer
    let mut alone = message("Hi");
    alone["usage"]["iterations"] =
        json!([{"type": "message", "model": "claude-opus-5", "input_tokens": 12, "output_tokens": 3}]);
    let (url, _) = serve(vec![(200, vec![], reply), (200, vec![], alone)]);
    let model = ModelConfig::new("anthropic", "claude-opus-5");
    let response = client(&url).complete(&request(&model)).unwrap();
    assert_eq!((response.model.as_str(), response.usage.output_tokens), ("claude-opus-4-8", 264));
    assert_eq!(response.declined.len(), 1);
    let declined = &response.declined[0];
    assert_eq!(declined.model, "claude-opus-5");
    assert_eq!(
        (declined.usage.input_tokens, declined.usage.output_tokens, declined.usage.cache_read_input_tokens),
        (400, 2000, 100)
    );
    assert!(client(&url).complete(&request(&model)).unwrap().declined.is_empty());
}

#[test]
fn an_agent_s_prefix_is_cached_and_what_the_cache_did_is_read_back() {
    let mut reply = message("ok");
    reply["usage"] = json!({
        "input_tokens": 40,
        "output_tokens": 3,
        "cache_creation_input_tokens": 600,
        "cache_read_input_tokens": 2_000,
        "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 600}
    });
    let (url, received) = serve(vec![(200, vec![], reply), (200, vec![], message("ok"))]);
    let mut model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    model.cache_ttl = CacheTtl::OneHour;
    let usage = client(&url).complete(&request(&model)).unwrap().usage;
    assert_eq!(
        (usage.input_tokens, usage.cache_creation_input_tokens, usage.cache_creation_1h_input_tokens),
        (40, 600, 600)
    );
    assert_eq!((usage.cache_read_input_tokens, usage.prompt_tokens()), (2_000, 2_640));
    model.cache = Caching::Off;
    client(&url).complete(&request(&model)).unwrap();

    let received = received.lock().unwrap();
    let hour = json!({"type": "ephemeral", "ttl": "1h"});
    let cached = &received[0].body;
    assert_eq!(cached["tools"][0]["cache_control"], hour);
    assert_eq!(cached["system"], json!([{"type": "text", "text": "be brief", "cache_control": hour}]));
    assert_eq!(cached["messages"][0]["content"], json!([{"type": "text", "text": "Hello", "cache_control": hour}]));
    // caching needs no beta header any more
    assert!(!received[0].headers.contains_key("anthropic-beta"));
    let plain = &received[1].body;
    assert!(!plain.to_string().contains("cache_control"), "{plain}");
    assert_eq!(plain["system"], "be brief");
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

#[test]
fn a_batch_is_submitted_polled_and_read_in_order() {
    let lines = [
        json!({"custom_id": "r1", "result": {"type": "succeeded", "message": message("second")}}),
        json!({"custom_id": "r0", "result": {"type": "succeeded", "message": message("first")}}),
        json!({"custom_id": "r2", "result": {"type": "errored", "error": {"error": {"message": "invalid request"}}}}),
    ];
    let jsonl: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let (url, received) = serve(vec![
        (200, vec![], json!({"id": "b1", "processing_status": "in_progress"})),
        (200, vec![], json!({"id": "b1", "processing_status": "in_progress"})),
        (200, vec![], json!({"id": "b1", "processing_status": "ended", "results_url": "BASE/results/b1"})),
        (200, vec![], Json::String(jsonl)),
    ]);
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let requests = [request(&model), request(&model), request(&model)];
    let client = client(&url).with_poll_interval(Duration::from_millis(5));
    let results = client.batch(&requests).unwrap();
    assert_eq!(results[0].as_ref().unwrap().text(), "first");
    assert_eq!(results[1].as_ref().unwrap().text(), "second");
    assert_eq!(results[2].as_ref().unwrap_err().message, "invalid request");
    let received = received.lock().unwrap();
    let paths: Vec<&str> = received.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, ["/v1/messages/batches", "/v1/messages/batches/b1", "/v1/messages/batches/b1", "/results/b1"]);
    let items = received[0].body["requests"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[2]["custom_id"], "r2");
    assert_eq!(items[0]["params"]["model"], "claude-haiku-4-5");
    assert_eq!(received[3].headers["x-api-key"], received[0].headers["x-api-key"]);
}

#[test]
fn a_batch_never_asks_for_server_side_fallback() {
    let jsonl = format!("{}\n", json!({"custom_id": "r0", "result": {"type": "succeeded", "message": message("ok")}}));
    let (url, received) = serve(vec![
        (200, vec![], json!({"id": "b1", "processing_status": "ended", "results_url": "BASE/results/b1"})),
        (200, vec![], Json::String(jsonl)),
    ]);
    // fallbacks by default for claude-opus-5, sent to the Messages API
    let model = ModelConfig::new("anthropic", "claude-opus-5");
    assert!(model.fallbacks);
    let client = client(&url).with_poll_interval(Duration::from_millis(5));
    assert_eq!(client.batch(&[request(&model)]).unwrap()[0].as_ref().unwrap().text(), "ok");
    let received = received.lock().unwrap();
    let params = &received[0].body["requests"][0]["params"];
    assert_eq!(params["model"], "claude-opus-5");
    assert!(params.get("fallbacks").is_none(), "{params}");
    assert!(!received[0].headers.contains_key("anthropic-beta"));
}
