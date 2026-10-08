//! The exporter against a fake OTLP/HTTP collector: spans arrive as OTLP
//! JSON with the configured headers; a collector down or slow never makes
//! the program wait; a full queue drops spans and counts them.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use grenat_otel::{Batching, Config, Exporter, Kind, SpanStart, Tracer, Transport};

/// What the fake collector received: (headers, JSON body).
type Received = Arc<Mutex<Vec<(Vec<(String, String)>, serde_json::Value)>>>;

/// A collector on a free port, answering `200` after `delay`.
fn collector(delay: Duration) -> (String, Received) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let received: Received = Arc::default();
    let kept = received.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let kept = kept.clone();
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
    fn send(&self, body: Vec<u8>) -> Result<(), String> {
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        *self.0.lock().unwrap() += json["resourceSpans"][0]["scopeSpans"][0]["spans"].as_array().unwrap().len();
        Ok(())
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
