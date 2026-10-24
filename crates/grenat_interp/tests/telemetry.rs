//! OpenTelemetry: the spans a run and a server export to a fake OTLP/HTTP
//! collector — their structure, parentage across tasks and processes,
//! their GenAI attributes — and what they never carry (a secret, a
//! prompt's text, a request's path); a collector down, slow or a queue
//! full never changes what the program does.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use grenat_interp::{Options, Output, Response, Scripted, run_main};
use grenat_otel::{Config, Tracer};
use serde_json::{Value as Json, json};

/// A request a fake server received: its headers and body.
#[derive(Clone, Debug)]
struct Received {
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// A server on a free port answering `200 {}` after `delay`: a collector,
/// or a service an `Http` call reaches.
struct FakeServer {
    url: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl FakeServer {
    fn start(delay: Duration) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let received: Arc<Mutex<Vec<Received>>> = Arc::default();
        let kept = received.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let kept = kept.clone();
                std::thread::spawn(move || answer(stream, delay, &kept));
            }
        });
        FakeServer { url, received }
    }

    fn requests(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    /// Every span the collector received, in order.
    fn spans(&self) -> Vec<Json> {
        let mut spans = Vec::new();
        for request in self.requests() {
            let body: Json = serde_json::from_slice(&request.body).unwrap();
            for resource in body["resourceSpans"].as_array().unwrap() {
                for scope in resource["scopeSpans"].as_array().unwrap() {
                    spans.extend(scope["spans"].as_array().unwrap().iter().cloned());
                }
            }
        }
        spans
    }

    /// Everything the collector received, as text.
    fn everything(&self) -> String {
        self.requests().iter().map(|r| String::from_utf8_lossy(&r.body).into_owned()).collect()
    }
}

fn answer(stream: TcpStream, delay: Duration, received: &Mutex<Vec<Received>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
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
    received.lock().unwrap().push(Received { headers, body });
    let mut stream = stream;
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}");
}

fn tracer(collector: &str, configure: impl FnOnce(&mut Config)) -> Arc<Tracer> {
    let mut config = Config::new(format!("{collector}/v1/traces"), "support-agents");
    config.delay = Duration::from_millis(50);
    config.timeout = Duration::from_secs(5);
    configure(&mut config);
    Arc::new(Tracer::new(&config).unwrap())
}

/// Runs `src` (its model answering `replies`) with spans going to `tracer`.
fn traced_run(src: &str, replies: Vec<Response>, tracer: Arc<Tracer>) -> (Result<i32, String>, String, Duration) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let output = Arc::new(Mutex::new(String::new()));
    let options = Options {
        provider: Some(Arc::new(Scripted::new(replies))),
        output: Output::Capture(output.clone()),
        journal: Some(temp_dir("journal")),
        telemetry: Some(tracer),
        ..Options::default()
    };
    let started = Instant::now();
    let result = run_main(&parsed.program, Vec::new(), options);
    let elapsed = started.elapsed();
    let printed = output.lock().unwrap().clone();
    (result.map(|s| s.exit_code).map_err(|e| format!("{}: {}", e.ty, e.message)), printed, elapsed)
}

fn named<'a>(spans: &'a [Json], name: &str) -> Vec<&'a Json> {
    spans.iter().filter(|s| s["name"] == name).collect()
}

fn one<'a>(spans: &'a [Json], name: &str) -> &'a Json {
    let found = named(spans, name);
    assert_eq!(found.len(), 1, "spans named {name:?}: {:#?}", spans.iter().map(|s| &s["name"]).collect::<Vec<_>>());
    found[0]
}

/// The value of an attribute, as OTLP JSON writes it.
fn attr<'a>(span: &'a Json, key: &str) -> Option<&'a Json> {
    let attributes = span["attributes"].as_array()?;
    let value = &attributes.iter().find(|a| a["key"] == key)?["value"];
    value.as_object()?.values().next()
}

fn parent_of(span: &Json) -> &str {
    span["parentSpanId"].as_str().unwrap_or("")
}

const AGENT_APP: &str = "\
model :smart, provider: :anthropic, name: \"claude-opus-5\", price: {input: 3, output: 15}
struct Report
  answer: String
end
## Reads a file.
tool read_file(path: String) -> String
  \"contents of #{path}\"
end
agent Reader
  model :smart
  tools read_file
  instructions \"You read the private ledger of ACME.\"
  on Ask(question: String) -> ~Report
    run \"Question: #{question}\"
  end
end
prompt summarize(text: String) -> ~String using :smart
  user \"Summarize the confidential memo: #{text}\"
end
workflow review(topic: String) -> String
  reader = spawn Reader
  report = step(:read) { reader.ask(Ask(question: \"what does #{topic} say?\")).trust!.answer }
  step(:publish) { \"published #{report}\" }
end
def main
  mock_credentials({\"crm\" => {\"token\" => \"tok-9f8e7d6c5b4a\"}})
  puts review(\"payroll\")
  summaries = [\"memo one\", \"memo two\"].parallel_map { |m| summarize(m).trust! }
  res = Http.get(\"SERVICE/orders/1234?customer=alice\", headers: {\"Authorization\" => Credentials.fetch(:crm, :token)})
  puts res.status
end
";

fn agent_replies() -> Vec<Response> {
    vec![
        Response::tool_call("call-1", "read_file", json!({"path": "ledger.txt"})).with_usage(1200, 40),
        Response::tool_call("call-2", "final_answer", json!({"answer": "all good"})).with_usage(1300, 20),
        Response::text_reply("short one"),
        Response::text_reply("short two"),
    ]
}

#[test]
fn a_run_exports_its_workflow_agent_tool_and_model_spans_in_one_trace() {
    let collector = FakeServer::start(Duration::ZERO);
    let service = FakeServer::start(Duration::ZERO);
    let src = AGENT_APP.replace("SERVICE", &service.url);
    let (result, output, _) = traced_run(&src, agent_replies(), tracer(&collector.url, |_| {}));
    assert_eq!(result, Ok(0), "{output}");
    assert!(output.contains("published all good") && output.contains("200"), "{output}");
    let spans = collector.spans();

    let root = one(&spans, "grenat run");
    assert_eq!(parent_of(root), "");
    let trace = root["traceId"].as_str().unwrap();
    assert!(spans.iter().all(|s| s["traceId"] == trace), "one trace");

    let workflow = one(&spans, "invoke_workflow review");
    assert_eq!(parent_of(workflow), root["spanId"]);
    assert_eq!(attr(workflow, "gen_ai.operation.name").unwrap(), "invoke_workflow");
    assert_eq!(attr(workflow, "gen_ai.workflow.name").unwrap(), "review");
    let read = one(&spans, "step read");
    assert_eq!(parent_of(read), workflow["spanId"]);
    assert_eq!(attr(read, "grenat.step.replayed").unwrap(), false);
    assert_eq!(parent_of(one(&spans, "step publish")), workflow["spanId"]);

    let ask = one(&spans, "invoke_agent Reader");
    assert_eq!(parent_of(ask), read["spanId"]);
    assert_eq!(attr(ask, "gen_ai.agent.name").unwrap(), "Reader");
    assert_eq!(attr(ask, "grenat.agent.message").unwrap(), "Ask");

    let chats = named(&spans, "chat claude-opus-5");
    assert_eq!(chats.len(), 4);
    let turns: Vec<&&Json> = chats.iter().filter(|c| parent_of(c) == ask["spanId"]).collect();
    assert_eq!(turns.len(), 2, "the agent's two turns are children of its ask");
    let first = turns.iter().find(|c| attr(c, "gen_ai.usage.input_tokens") == Some(&json!("1200"))).unwrap();
    assert_eq!(first["kind"], 3);
    assert_eq!(attr(first, "gen_ai.operation.name").unwrap(), "chat");
    assert_eq!(attr(first, "gen_ai.provider.name").unwrap(), "anthropic");
    assert_eq!(attr(first, "gen_ai.request.model").unwrap(), "claude-opus-5");
    assert_eq!(attr(first, "gen_ai.response.model").unwrap(), "scripted");
    assert_eq!(attr(first, "gen_ai.usage.output_tokens").unwrap(), "40");
    assert_eq!(attr(first, "gen_ai.agent.name").unwrap(), "Reader");
    assert_eq!(attr(first, "gen_ai.response.finish_reasons").unwrap()["values"][0]["stringValue"], "tool_use");
    // 1200 tokens in at $3 a million, 40 out at $15
    let cost = attr(first, "grenat.usage.cost_usd").unwrap().as_f64().unwrap();
    assert!((cost - 0.0042).abs() < 1e-9, "{cost}");
    assert!(attr(first, "grenat.prompt.bytes").unwrap().as_str().unwrap().parse::<u64>().unwrap() > 0);

    let tool = one(&spans, "execute_tool read_file");
    assert_eq!(parent_of(tool), ask["spanId"]);
    assert_eq!(attr(tool, "gen_ai.tool.name").unwrap(), "read_file");
    assert_eq!(attr(tool, "gen_ai.tool.call.id").unwrap(), "call-1");
    assert_eq!(attr(tool, "gen_ai.tool.type").unwrap(), "function");

    // the calls of `parallel_map`, on tasks of their own, are children of the span it ran in
    let summaries: Vec<&&Json> = chats.iter().filter(|c| parent_of(c) == root["spanId"]).collect();
    assert_eq!(summaries.len(), 2);

    let http = one(&spans, "GET");
    assert_eq!(http["kind"], 3);
    assert_eq!(parent_of(http), root["spanId"]);
    assert_eq!(attr(http, "http.response.status_code").unwrap(), "200");
    assert_eq!(attr(http, "server.address").unwrap(), "127.0.0.1");
    // the service called continues the trace
    let called = &service.requests()[0];
    let traceparent = called.header("traceparent").expect("a traceparent sent");
    assert_eq!(traceparent, format!("00-{trace}-{}-01", http["spanId"].as_str().unwrap()));

    // never a secret, a prompt, an answer, a URL's path or query
    let everything = collector.everything();
    for leak in [
        "tok-9f8e7d6c5b4a",
        "private ledger",
        "Question:",
        "payroll",
        "confidential memo",
        "memo one",
        "short one",
        "all good",
        "contents of",
        "ledger.txt",
        "/orders/1234",
        "alice",
    ] {
        assert!(!everything.contains(leak), "{leak:?} was exported");
    }
}

#[test]
fn errors_are_recorded_by_their_type_and_their_message_only_when_content_is_captured() {
    let src = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt greet(name: String) -> ~String using :fast
  user \"Say hello to #{name}\"
end
def main
  mock_credentials({\"crm\" => {\"token\" => \"tok-9f8e7d6c5b4a\"}})
  puts greet(\"Ada\").trust!
  raise \"cannot reach the CRM with tok-9f8e7d6c5b4a\"
end
";
    let plain = FakeServer::start(Duration::ZERO);
    let (result, _, _) = traced_run(src, vec![Response::text_reply("Hello Ada")], tracer(&plain.url, |_| {}));
    assert!(result.is_err());
    let spans = plain.spans();
    let root = one(&spans, "grenat run");
    assert_eq!(root["status"]["code"], 2);
    let event = &root["events"][0];
    assert_eq!(event["name"], "exception");
    assert_eq!(attr(event, "exception.type").unwrap(), "RuntimeError");
    assert!(attr(event, "exception.message").is_none());
    assert_eq!(attr(root, "error.type").unwrap(), "RuntimeError");
    assert!(!plain.everything().contains("cannot reach"));
    assert!(!plain.everything().contains("Say hello"));

    let captured = FakeServer::start(Duration::ZERO);
    let tracer = tracer(&captured.url, |c| c.capture_content = true);
    let (result, _, _) = traced_run(src, vec![Response::text_reply("Hello Ada")], tracer);
    assert!(result.is_err());
    let spans = captured.spans();
    let event = &one(&spans, "grenat run")["events"][0];
    assert_eq!(attr(event, "exception.message").unwrap(), "cannot reach the CRM with [secret]");
    let chat = one(&spans, "chat claude-haiku-4-5");
    let input: Json = serde_json::from_str(attr(chat, "gen_ai.input.messages").unwrap().as_str().unwrap()).unwrap();
    assert_eq!(input[0]["parts"][0]["content"], "Say hello to Ada");
    let output: Json = serde_json::from_str(attr(chat, "gen_ai.output.messages").unwrap().as_str().unwrap()).unwrap();
    assert_eq!(output[0]["parts"][0]["content"], "Hello Ada");
    assert!(!captured.everything().contains("tok-9f8e7d6c5b4a"));
}

#[test]
fn ssh_calls_are_spans_of_their_own() {
    let collector = FakeServer::start(Duration::ZERO);
    let src = "\
def main
  mock_ssh \"deploy@api.acme.com\", commands: {\"systemctl restart shop\" => \"done\"}
  server = Ssh.connect(\"deploy@api.acme.com\", password: \"pw-1234567\")
  puts server.run([\"systemctl\", \"restart\", \"shop\"]).ok?
end
";
    let (result, output, _) = traced_run(src, Vec::new(), tracer(&collector.url, |_| {}));
    assert_eq!(result, Ok(0), "{output}");
    let spans = collector.spans();
    let connect = one(&spans, "ssh connect");
    assert_eq!(attr(connect, "server.address").unwrap(), "api.acme.com");
    let run = one(&spans, "ssh run");
    assert_eq!(run["kind"], 3);
    assert_eq!(attr(run, "process.exit.code").unwrap(), "0");
    assert_eq!(parent_of(run), one(&spans, "grenat run")["spanId"]);
    assert!(!collector.everything().contains("pw-1234567"));
    assert!(!collector.everything().contains("systemctl"));
}

const BUSY: &str = "\
workflow ticks(n: Int) -> Int
  [1, 2, 3, 4, 5, 6, 7, 8, 9, 10].each { |i| step(:tick) { i } }
  n
end
def main
  puts ticks(1)
end
";

#[test]
fn a_collector_down_changes_nothing_but_a_warning() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let (result, output, elapsed) = traced_run(BUSY, Vec::new(), tracer(&closed, |_| {}));
    assert_eq!(result, Ok(0));
    assert!(output.starts_with("1\n"), "{output}");
    assert!(output.contains("warning: OpenTelemetry: spans 12 not exported (collector unreachable"), "{output}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

#[test]
fn a_slow_collector_delays_the_end_of_a_run_by_its_timeout_at_most() {
    let slow = FakeServer::start(Duration::from_secs(30));
    let tracer = tracer(&slow.url, |c| c.timeout = Duration::from_millis(300));
    let (result, output, elapsed) = traced_run(BUSY, Vec::new(), tracer);
    assert_eq!(result, Ok(0));
    assert!(output.starts_with("1\n"), "{output}");
    assert!(output.contains("not exported"), "{output}");
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
}

#[test]
fn a_full_queue_drops_spans_and_says_how_many() {
    let collector = FakeServer::start(Duration::ZERO);
    let tracer = tracer(&collector.url, |c| {
        c.max_queue = 4;
        c.max_batch = 100;
        c.delay = Duration::from_secs(3600);
    });
    let (result, output, _) = traced_run(BUSY, Vec::new(), tracer);
    assert_eq!(result, Ok(0));
    assert!(output.contains("warning: OpenTelemetry: spans 8 dropped (the queue was full)"), "{output}");
    assert_eq!(collector.spans().len(), 4);
}

#[test]
fn nothing_is_exported_without_a_tracer() {
    let run = run_with(BUSY, Vec::new(), &[]);
    assert_eq!(run.ok(), "1\n");
}

/// Serves `src` with spans going to `tracer`: the address it listens on.
fn serve(src: &str, tracer: Arc<Tracer>) -> String {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let program: &'static grenat_ast::Program = Box::leak(Box::new(parsed.program));
    let options = Options {
        output: Output::Capture(Arc::default()),
        journal: Some(temp_dir("journal")),
        telemetry: Some(tracer),
        ..Options::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        grenat_interp::serve(program, options, "127.0.0.1:0", move |address| tx.send(address.to_string()).unwrap())
    });
    rx.recv_timeout(Duration::from_secs(20)).expect("listening")
}

fn request(address: &str, method: &str, path: &str, headers: &[(&str, &str)]) -> u16 {
    let mut stream = TcpStream::connect(address).unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Length: 0\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer.split(' ').nth(1).unwrap().parse().unwrap()
}

fn wait_for(collector: &FakeServer, done: impl Fn(&[Json]) -> bool) -> Vec<Json> {
    let started = Instant::now();
    loop {
        let spans = collector.spans();
        if done(&spans) {
            return spans;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "timed out: {spans:#?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn served_requests_jobs_and_schedules_are_traced() {
    let collector = FakeServer::start(Duration::ZERO);
    let database = temp_dir("telemetry").join("app.db");
    let src = format!(
        "\
database \"sqlite://{}\"
def digest(n: Int)
  n * 2
end
def broken(n: Int)
  raise \"no such ticket {{n}}\"
end
get \"/tickets/:id\" do |req|
  enqueue(:digest, 3)
  \"ticket\"
end
get \"/quiet\" do |req|
  \"not sampled\"
end
get \"/crash\" do |req|
  enqueue(:broken, 1)
  raise \"crashed on customer data\"
end
every 1 do
  1
end
",
        database.display()
    );
    let address = serve(&src, tracer(&collector.url, |_| {}));
    // a caller that does not sample its trace: nothing of it is exported
    let unsampled = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-00";
    assert_eq!(request(&address, "GET", "/quiet", &[("traceparent", unsampled)]), 200);
    let caller = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    assert_eq!(request(&address, "GET", "/tickets/q4242z?secret=alice", &[("traceparent", caller)]), 200);
    assert_eq!(request(&address, "GET", "/crash", &[]), 500);
    let spans = wait_for(&collector, |spans| {
        ["GET /tickets/:id", "GET /crash", "job digest", "job broken", "schedule every 1s"]
            .iter()
            .all(|name| spans.iter().any(|s| s["name"] == *name))
    });

    assert!(named(&spans, "GET /quiet").is_empty(), "an unsampled trace was exported");
    assert!(!collector.everything().contains("0af7651916cd43dd8448eb211c80319c"));
    let ticket = &named(&spans, "GET /tickets/:id")[0];
    assert_eq!(ticket["kind"], 2);
    assert_eq!(ticket["traceId"], "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(parent_of(ticket), "00f067aa0ba902b7");
    assert_eq!(attr(ticket, "http.route").unwrap(), "/tickets/:id");
    assert_eq!(attr(ticket, "http.request.method").unwrap(), "GET");
    assert_eq!(attr(ticket, "http.response.status_code").unwrap(), "200");
    assert_eq!(ticket["status"], json!({}));

    let crash = &named(&spans, "GET /crash")[0];
    assert_eq!(parent_of(crash), "");
    assert_eq!(attr(crash, "http.response.status_code").unwrap(), "500");
    assert_eq!(crash["status"]["code"], 2);
    assert_eq!(attr(&crash["events"][0], "exception.type").unwrap(), "RuntimeError");

    let job = &named(&spans, "job digest")[0];
    assert_eq!(job["kind"], 5);
    assert_eq!(parent_of(job), "");
    assert_eq!(attr(job, "grenat.job.name").unwrap(), "digest");
    let broken = &named(&spans, "job broken")[0];
    assert_eq!(broken["status"]["code"], 2);

    let schedule = &named(&spans, "schedule every 1s")[0];
    assert_eq!(attr(schedule, "grenat.trigger").unwrap(), "schedule");

    // markers no exported number can hold by chance (ids are hex, times decimal)
    let everything = collector.everything();
    for leak in ["q4242z", "alice", "crashed on customer data", "no such ticket"] {
        assert!(!everything.contains(leak), "{leak:?} was exported");
    }
}
