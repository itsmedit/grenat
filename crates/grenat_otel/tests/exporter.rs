//! The exporter against a fake OTLP/HTTP collector: spans arrive as OTLP
//! JSON with the configured headers; a collector down or slow never makes
//! the program wait; a full queue drops spans and counts them; a redirect
//! is never followed (the API keys stay home); `429`, `503` and a
//! collector unreachable are sent again, after `Retry-After` if given,
//! other answers not; a partial success counts its rejected spans; an
//! unsampled parent's spans are not exported; an idle exporter sleeps.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use grenat_otel::{Batching, Config, Export, Exporter, Kind, SpanContext, SpanStart, Tracer, Transport};

/// What the fake collector received: (headers, JSON body).
type Received = Arc<Mutex<Vec<(Vec<(String, String)>, serde_json::Value)>>>;

/// A collector on a free port, answering `200`: the first request after
/// `delay`, the next ones at once.
fn collector(delay: Duration) -> (String, Received) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let received: Received = Arc::default();
    let kept = received.clone();
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().flatten().enumerate() {
            let kept = kept.clone();
            let delay = if n == 0 { delay } else { Duration::ZERO };
            std::thread::spawn(move || answer(stream, delay, &kept));
        }
    });
    (format!("http://{address}/v1/traces"), received)
}

fn answer(stream: TcpStream, delay: Duration, received: &Received) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap();
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let length: usize = headers.iter().find(|(n, _)| n == "content-length").map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    std::thread::sleep(delay);
    received.lock().unwrap().push((headers, serde_json::from_slice(&body).unwrap()));
    let mut stream = stream;
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}");
}

/// An address nobody listens on.
fn closed_port() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{address}/v1/traces")
}

fn config(endpoint: &str) -> Config {
    Config {
        timeout: Duration::from_millis(300),
        delay: Duration::from_millis(50),
        ..Config::new(endpoint, "test-service")
    }
}

fn spans(received: &Received) -> Vec<serde_json::Value> {
    let received = received.lock().unwrap();
    received
        .iter()
        .flat_map(|(_, body)| {
            body["resourceSpans"][0]["scopeSpans"][0]["spans"].as_array().cloned().unwrap_or_default()
        })
        .collect()
}

#[test]
fn spans_reach_the_collector_as_otlp_json_with_their_parents_and_headers() {
    let (endpoint, received) = collector(Duration::ZERO);
    let config = Config { headers: vec![("x-api-key".into(), "k-123".into())], ..config(&endpoint) };
    let tracer = Tracer::new(&config).unwrap();
    let root = tracer.start(SpanStart::new("job digest", Kind::Consumer), None);
    let mut child = tracer.start(
        SpanStart::new("chat claude", Kind::Client).attr("gen_ai.usage.input_tokens", 12_i64),
        Some(root.context),
    );
    child.fail("LlmError", None);
    let (root_id, trace_id) = (root.context.span_id.hex(), root.context.trace_id.hex());
    tracer.end(child);
    tracer.end(root);
    assert!(tracer.shutdown(Duration::from_secs(5)));
    let spans = spans(&received);
    assert_eq!(spans.len(), 2);
    let (child, root) = (&spans[0], &spans[1]);
    assert_eq!(root["spanId"], root_id.as_str());
    assert!(root.get("parentSpanId").is_none());
    assert_eq!(child["parentSpanId"], root_id.as_str());
    assert_eq!(child["traceId"], trace_id.as_str());
    assert_eq!(child["kind"], 3);
    assert_eq!(child["status"]["code"], 2);
    assert!(child["startTimeUnixNano"].as_str().unwrap().parse::<u64>().unwrap() > 0);
    let received = received.lock().unwrap();
    let (headers, body) = &received[0];
    assert!(headers.contains(&("x-api-key".into(), "k-123".into())));
    assert!(headers.contains(&("content-type".into(), "application/json".into())));
    let resource = &body["resourceSpans"][0]["resource"]["attributes"][0];
    assert_eq!(resource["value"]["stringValue"], "test-service");
    assert_eq!(tracer.stats().exported, 2);
}

#[test]
fn a_collector_down_costs_the_program_nothing_and_is_counted() {
    let tracer = Tracer::new(&config(&closed_port())).unwrap();
    let started = Instant::now();
    for i in 0..1000 {
        tracer.end(tracer.start(SpanStart::new(format!("step {i}"), Kind::Internal), None));
    }
    assert!(started.elapsed() < Duration::from_millis(500), "ending spans waited: {:?}", started.elapsed());
    assert!(tracer.flush(Duration::from_secs(10)));
    let stats = tracer.stats();
    assert_eq!((stats.exported, stats.failed, stats.dropped), (0, 1000, 0));
    // two batches, each sent four times
    assert_eq!(stats.retries, 6);
    assert!(stats.last_error.unwrap().contains("unreachable"));
}

#[test]
fn a_slow_collector_never_holds_the_program_and_a_flush_waits_its_time_only() {
    let (endpoint, received) = collector(Duration::from_secs(3));
    let config = Config { timeout: Duration::from_secs(10), ..config(&endpoint) };
    let tracer = Tracer::new(&config).unwrap();
    tracer.end(tracer.start(SpanStart::new("first", Kind::Internal), None));
    std::thread::sleep(Duration::from_millis(200));
    // the exporter is stuck sending the first batch: the program goes on
    let started = Instant::now();
    for _ in 0..1000 {
        tracer.end(tracer.start(SpanStart::new("next", Kind::Internal), None));
    }
    assert!(started.elapsed() < Duration::from_millis(500), "ending spans waited: {:?}", started.elapsed());
    let started = Instant::now();
    assert!(!tracer.flush(Duration::from_millis(300)));
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(300) && waited < Duration::from_secs(2), "{waited:?}");
    // given the time, everything arrives
    assert!(tracer.flush(Duration::from_secs(20)));
    assert_eq!(spans(&received).len(), 1001);
}

#[test]
fn an_export_that_times_out_is_counted_as_failed() {
    let (endpoint, _received) = collector(Duration::from_secs(5));
    let tracer = Tracer::new(&config(&endpoint)).unwrap();
    tracer.end(tracer.start(SpanStart::new("late", Kind::Internal), None));
    let started = Instant::now();
    assert!(tracer.flush(Duration::from_secs(4)));
    assert!(started.elapsed() < Duration::from_secs(3), "the export's timeout bounds it: {:?}", started.elapsed());
    assert_eq!(tracer.stats().failed, 1);
}

/// A transport that only counts what it is given.
struct Counting(Arc<Mutex<usize>>);

impl Transport for Counting {
    fn send(&self, body: &[u8]) -> Export {
        let json: serde_json::Value = serde_json::from_slice(body).unwrap();
        *self.0.lock().unwrap() += json["resourceSpans"][0]["scopeSpans"][0]["spans"].as_array().unwrap().len();
        Export::Accepted { rejected: 0, message: None }
    }
}

#[test]
fn a_full_queue_drops_spans_and_counts_them() {
    let sent = Arc::new(Mutex::new(0));
    // the exporter waits an hour or a flush: the queue fills up
    let batching = Batching { max_queue: 10, max_batch: 1000, delay: Duration::from_secs(3600) };
    let exporter = Exporter::start(Vec::new(), Box::new(Counting(sent.clone())), batching).unwrap();
    let tracer_config = Config::new("http://unused", "x");
    let tracer = Tracer::with_transport(&tracer_config, Box::new(Counting(Arc::default()))).unwrap();
    for i in 0..100 {
        exporter.push(tracer.start(SpanStart::new(format!("span {i}"), Kind::Internal), None));
    }
    assert_eq!(exporter.stats().dropped, 90);
    assert!(exporter.shutdown(Duration::from_secs(5)));
    assert_eq!(*sent.lock().unwrap(), 10);
    assert_eq!(exporter.stats().exported, 10);
    // once shut down, nothing more is queued
    exporter.push(tracer.start(SpanStart::new("late", Kind::Internal), None));
    assert_eq!(exporter.stats().dropped, 91);
}

/// A collector answering each request with the next of `answers` (status
/// line, extra headers, body), then `200`: how many requests it got.
fn scripted(answers: Vec<(&'static str, String, &'static str)>) -> (String, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let count = Arc::new(Mutex::new(0));
    let counted = count.clone();
    std::thread::spawn(move || {
        let mut answers = answers.into_iter();
        for stream in listener.incoming().flatten() {
            let (status, headers, body) = answers.next().unwrap_or(("200 OK", String::new(), "{}"));
            *counted.lock().unwrap() += 1;
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim_end().is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut request_body = vec![0; length];
            let _ = reader.read_exact(&mut request_body);
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (format!("http://{address}/v1/traces"), count)
}

#[test]
fn a_redirect_is_never_followed_with_the_api_keys() {
    let (elsewhere, received) = collector(Duration::ZERO);
    let (endpoint, asked) = scripted(vec![("302 Found", format!("Location: {elsewhere}\r\n"), "")]);
    let config = Config { headers: vec![("x-honeycomb-team".into(), "key".into())], ..config(&endpoint) };
    let tracer = Tracer::new(&config).unwrap();
    tracer.end(tracer.start(SpanStart::new("moved", Kind::Internal), None));
    assert!(tracer.flush(Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(200));
    assert!(received.lock().unwrap().is_empty(), "the redirect was followed");
    assert_eq!(*asked.lock().unwrap(), 1, "a redirect is not sent again");
    let stats = tracer.stats();
    assert_eq!((stats.exported, stats.failed, stats.retries), (0, 1, 0));
    assert!(stats.last_error.unwrap().contains("302"));
}

#[test]
fn a_busy_collector_is_sent_the_batch_again_after_its_retry_after() {
    let (endpoint, asked) = scripted(vec![
        ("503 Service Unavailable", "Retry-After: 1\r\n".into(), ""),
        ("429 Too Many Requests", String::new(), ""),
    ]);
    let tracer = Tracer::new(&config(&endpoint)).unwrap();
    let started = Instant::now();
    tracer.end(tracer.start(SpanStart::new("again", Kind::Internal), None));
    assert!(tracer.flush(Duration::from_secs(10)));
    // a second waited, then a short backoff
    assert!(started.elapsed() >= Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(*asked.lock().unwrap(), 3);
    let stats = tracer.stats();
    assert_eq!((stats.exported, stats.failed, stats.retries), (1, 0, 2));
}

#[test]
fn other_answers_and_long_retry_afters_are_not_sent_again() {
    for (status, headers) in
        [("400 Bad Request", ""), ("401 Unauthorized", ""), ("503 Service Unavailable", "Retry-After: 3600\r\n")]
    {
        let (endpoint, asked) = scripted(vec![(status, headers.to_string(), "")]);
        let tracer = Tracer::new(&config(&endpoint)).unwrap();
        tracer.end(tracer.start(SpanStart::new("once", Kind::Internal), None));
        assert!(tracer.flush(Duration::from_secs(5)));
        assert_eq!(*asked.lock().unwrap(), 1, "{status}");
        assert_eq!(tracer.stats().failed, 1, "{status}");
    }
}

#[test]
fn spans_rejected_in_a_partial_success_are_counted_as_dropped() {
    let partial = r#"{"partialSuccess": {"rejectedSpans": "1", "errorMessage": "span too old"}}"#;
    let (endpoint, _) = scripted(vec![("200 OK", "Content-Type: application/json\r\n".into(), partial)]);
    let tracer = Tracer::new(&config(&endpoint)).unwrap();
    tracer.end(tracer.start(SpanStart::new("a", Kind::Internal), None));
    tracer.end(tracer.start(SpanStart::new("b", Kind::Internal), None));
    assert!(tracer.flush(Duration::from_secs(5)));
    let stats = tracer.stats();
    assert_eq!((stats.exported, stats.dropped, stats.rejected, stats.failed), (1, 1, 1, 0));
    assert_eq!(stats.rejection.as_deref(), Some("span too old"));
}

#[test]
fn the_spans_of_an_unsampled_parent_are_not_exported_but_the_trace_goes_on() {
    let sent = Arc::new(Mutex::new(0));
    let tracer = Tracer::with_transport(&config("http://unused"), Box::new(Counting(sent.clone()))).unwrap();
    let caller = SpanContext::from_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00").unwrap();
    let request = tracer.start(SpanStart::new("GET /x", Kind::Server).remote_parent(Some(caller)), None);
    let child = tracer.start(SpanStart::new("chat", Kind::Client), Some(request.context));
    assert!(!request.context.sampled && !child.context.sampled);
    assert_eq!(child.context.trace_id, caller.trace_id);
    assert!(child.context.traceparent().ends_with("-00"), "{}", child.context.traceparent());
    tracer.end(child);
    tracer.end(request);
    // a sampled caller, or none: exported
    let sampled = SpanContext { sampled: true, ..caller };
    let other = tracer.start(SpanStart::new("GET /y", Kind::Server).remote_parent(Some(sampled)), None);
    assert!(other.context.sampled && other.context.traceparent().ends_with("-01"));
    tracer.end(other);
    let root = tracer.start(SpanStart::new("job", Kind::Consumer), None);
    assert!(root.context.sampled);
    tracer.end(root);
    assert!(tracer.flush(Duration::from_secs(5)));
    assert_eq!(*sent.lock().unwrap(), 2);
}

#[test]
fn an_idle_exporter_sleeps_even_with_no_delay() {
    let sent = Arc::new(Mutex::new(0));
    let batching = Batching { max_queue: 100, max_batch: 10, delay: Duration::ZERO };
    let exporter = Exporter::start(Vec::new(), Box::new(Counting(sent.clone())), batching).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(exporter.stats().rounds <= 2, "spinning: {} rounds", exporter.stats().rounds);
    let tracer = Tracer::with_transport(&config("http://unused"), Box::new(Counting(Arc::default()))).unwrap();
    exporter.push(tracer.start(SpanStart::new("one", Kind::Internal), None));
    assert!(exporter.flush(Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(*sent.lock().unwrap(), 1);
    assert!(exporter.stats().rounds <= 6, "spinning: {} rounds", exporter.stats().rounds);
}
