//! Rate limits: `limit net("host"), 10.per_second, concurrent: 4` shared by
//! every green task, calls over the limit waiting their turn (cancellable,
//! counted in a budget's time), model calls held by their provider's host,
//! and `429 Too Many Requests` answered with `Retry-After`. Timings are
//! asserted with generous margins: lower bounds the limit imposes, upper
//! bounds far above them.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::*;
use grenat_interp::{Response, Scripted};

/// The seconds a program printed last (`puts (Time.now - t)`).
fn seconds(output: &str) -> f64 {
    output.lines().filter_map(|l| l.trim().parse::<f64>().ok()).next_back().expect(output)
}

/// A local server: `/slow` answers after 100 ms (the most requests it
/// had in flight at once is counted), `/busy` answers `429` with
/// `Retry-After: 1` the first time, then `200`.
fn server(highest: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let current = Arc::new(AtomicUsize::new(0));
    let busy = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (current, highest, busy) = (current.clone(), highest.clone(), busy.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim_end().is_empty() {
                        break;
                    }
                }
                let (status, extra) = match path.as_str() {
                    "/slow" => {
                        let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                        highest.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(100));
                        current.fetch_sub(1, Ordering::SeqCst);
                        ("200 OK", "")
                    }
                    "/busy" if busy.fetch_add(1, Ordering::SeqCst) == 0 => {
                        ("429 Too Many Requests", "Retry-After: 1\r\n")
                    }
                    _ => ("200 OK", ""),
                };
                let mut stream = stream;
                let _ = write!(stream, "HTTP/1.1 {status}\r\n{extra}Content-Length: 2\r\nConnection: close\r\n\r\nok");
            });
        }
    });
    base
}

// ── A bucket shared by every task ─────────────────────────────────────

#[test]
fn many_green_tasks_share_one_bucket() {
    // 20 at once, then one each 50 ms: 30 calls take 500 ms at least
    let out = tests_pass(
        "limit net(\"api.x.io\"), 20.per_second
def fetch(i: Int) -> Int uses net(\"api.x.io\")
  Http.get(\"https://api.x.io/items/#{i}\").status
end
test \"thirty calls from thirty tasks\" do
  mock_http \"GET https://api.x.io/*\", body: \"ok\"
  t = Time.now
  codes = (1..30).to_a.parallel_map { |i| fetch(i) }
  assert_equal 30, codes.size
  assert_equal 30, Http.requests.size
  puts (Time.now - t)
end
",
    );
    let took = seconds(&out);
    assert!((0.45..10.0).contains(&took), "{took}");
}

#[test]
fn calls_under_the_rate_never_wait() {
    let out = tests_pass(
        "limit net(\"api.x.io\"), 100.per_second
test \"a burst within the bucket\" do
  mock_http \"GET https://api.x.io/*\", body: \"ok\"
  t = Time.now
  (1..50).to_a.parallel_map { |i| Http.get(\"https://api.x.io/#{i}\").status }
  puts (Time.now - t)
end
",
    );
    assert!(seconds(&out) < 0.4, "{out}");
}

#[test]
fn hosts_are_limited_apart_and_without_case() {
    // `API.X.IO` is `api.x.io`; `other.io` has no limit
    let out = tests_pass(
        "limit net(\"API.X.IO\"), 1.per_minute
test \"one call to the limited host, many to another\" do
  mock_http \"https://*\", body: \"ok\"
  t = Time.now
  Http.get(\"https://api.x.io/a\")
  (1..20).to_a.each { |i| Http.get(\"https://other.io/#{i}\") }
  puts (Time.now - t)
end
",
    );
    assert!(seconds(&out) < 1.0, "{out}");
}

#[test]
fn at_most_so_many_calls_in_flight() {
    let highest = Arc::new(AtomicUsize::new(0));
    let base = server(highest.clone());
    let run = run_with(
        &"limit net(\"127.0.0.1\"), concurrent: 2
codes = (1..8).to_a.parallel_map { |i| Http.get(\"BASE/slow\").status }
puts codes.sum
"
        .replace("BASE", &base),
        Vec::new(),
        &[],
    );
    let elapsed = run.elapsed;
    assert_eq!(run.ok().trim(), "1600");
    assert_eq!(highest.load(Ordering::SeqCst), 2);
    // four rounds of 100 ms
    assert!(elapsed >= Duration::from_millis(380), "{elapsed:?}");
}

#[test]
fn a_rate_and_a_concurrency_limit_together() {
    let highest = Arc::new(AtomicUsize::new(0));
    let base = server(highest.clone());
    let out = run(&"limit net(\"127.0.0.1\"), 4.per_second, concurrent: 3
t = Time.now
(1..6).to_a.parallel_map { |i| Http.get(\"BASE/slow\").status }
puts (Time.now - t)
"
    .replace("BASE", &base));
    assert!(highest.load(Ordering::SeqCst) <= 3);
    // 4 at once, then one each 250 ms
    assert!((0.45..10.0).contains(&seconds(&out)), "{out}");
}

// ── Waiting: cancellable, within budgets ──────────────────────────────

#[test]
fn a_cancelled_task_stops_waiting() {
    // the second call would wait a minute: the race is won long before
    let out = tests_pass(
        "def timer -> String
  sleep 0.2
  \"timer\"
end
test \"cancelled while waiting\" do
  limit net(\"slow.io\"), 1.per_minute
  mock_http \"GET https://slow.io/*\", body: \"ok\"
  t = Time.now
  w = race do
    Http.get(\"https://slow.io/a\").body + Http.get(\"https://slow.io/b\").body
    timer
  end
  assert_equal \"timer\", w
  assert_equal 1, Http.requests.size
  puts (Time.now - t)
end
",
    );
    assert!(seconds(&out) < 5.0, "{out}");
}

#[test]
fn waiting_is_time_a_budget_counts() {
    let out = tests_pass(
        "limit net(\"api.x.io\"), 10.per_second
test \"a budget's time runs out while calls wait\" do
  mock_http \"GET https://api.x.io/*\", body: \"ok\"
  t = Time.now
  stopped = false
  within budget(time: 1.s) do
    (1..100).to_a.each { |i| Http.get(\"https://api.x.io/#{i}\") }
  rescue BudgetExceeded => e
    stopped = true
  end
  assert stopped
  assert Http.requests.size < 30
  puts (Time.now - t)
end
",
    );
    assert!((0.9..5.0).contains(&seconds(&out)), "{out}");
}

// ── Model calls ──────────────────────────────────────────────────────

#[test]
fn model_calls_are_held_by_their_providers_host() {
    let replies: Vec<Response> = (0..12).map(|_| summary_reply()).collect();
    let run = run_with(
        &format!(
            "{SUMMARY}limit net(\"api.anthropic.com\"), 10.per_second
t = Time.now
(1..12).to_a.parallel_map {{ |i| summarize(\"article #{{i}}\") }}
puts (Time.now - t)
"
        ),
        replies,
        &[],
    );
    assert_eq!(run.requests.len(), 12);
    // ten at once, then two more 100 ms apart
    assert!((0.18..10.0).contains(&seconds(&run.ok())));
}

#[test]
fn a_mocked_model_sees_the_limit_in_tests() {
    let out = tests_pass(&format!(
        "{SUMMARY}limit net(\"api.anthropic.com\"), 5.per_second
test \"seven prompts\" do
  mock :fast, replies: (1..7).to_a.map {{ |i| {{title: \"t\", bullets: [], sentiment: \"Positive\"}} }}
  t = Time.now
  (1..7).to_a.parallel_map {{ |i| summarize(\"x\") }}
  puts (Time.now - t)
end
"
    ));
    assert!((0.35..10.0).contains(&seconds(&out)), "{out}");
}

#[test]
fn another_hosts_limit_leaves_models_alone() {
    let replies: Vec<Response> = (0..5).map(|_| summary_reply()).collect();
    let run = run_with(
        &format!(
            "{SUMMARY}limit net(\"api.openai.com\"), 1.per_minute
t = Time.now
(1..5).to_a.each {{ |i| summarize(\"x\") }}
puts (Time.now - t)
"
        ),
        replies,
        &[],
    );
    assert!(seconds(&run.ok()) < 1.0);
}

// ── 429 and Retry-After ──────────────────────────────────────────────

#[test]
fn a_429_with_retry_after_is_sent_again() {
    tests_pass(
        "test \"idempotent methods are retried\" do
  mock_http \"GET https://api.x.io/*\", body: \"ok\"
  mock_http \"GET https://api.x.io/*\", status: 429, headers: {\"Retry-After\" => \"0\"}, times: 2
  r = Http.get(\"https://api.x.io/a\")
  assert_equal 200, r.status
  assert_equal 3, Http.requests.size
end
test \"three retries at most\" do
  mock_http \"PUT https://api.x.io/*\", status: 429, headers: {\"Retry-After\" => \"0\"}
  assert_equal 429, Http.put(\"https://api.x.io/a\", json: {a: 1}).status
  assert_equal 4, Http.requests.size
end
test \"an HTTP date already past is no wait\" do
  mock_http \"GET https://api.x.io/*\", body: \"ok\"
  mock_http \"GET https://api.x.io/*\", status: 429, headers: {\"Retry-After\" => \"Sun, 06 Nov 1994 08:49:37 GMT\"}, times: 1
  assert_equal 200, Http.get(\"https://api.x.io/a\").status
end
test \"POST and PATCH are sent once unless told\" do
  mock_http \"https://api.x.io/*\", status: 429, headers: {\"Retry-After\" => \"0\"}
  assert_equal 429, Http.post(\"https://api.x.io/a\", json: {a: 1}).status
  assert_equal 429, Http.patch(\"https://api.x.io/a\", json: {a: 1}).status
  assert_equal 2, Http.requests.size
  Http.post(\"https://api.x.io/a\", json: {a: 1}, retry: true)
  assert_equal 6, Http.requests.size
end
test \"retry: false sends a GET once\" do
  mock_http \"GET https://api.x.io/*\", status: 429, headers: {\"Retry-After\" => \"0\"}
  assert_equal 429, Http.get(\"https://api.x.io/a\", retry: false).status
  assert_equal 1, Http.requests.size
end
test \"no Retry-After, or a long one: the program's answer\" do
  mock_http \"GET https://api.x.io/none\", status: 429
  mock_http \"GET https://api.x.io/long\", status: 429, headers: {\"Retry-After\" => \"3600\"}
  mock_http \"GET https://api.x.io/soon\", status: 503, headers: {\"Retry-After\" => \"0\"}
  assert_equal 429, Http.get(\"https://api.x.io/none\").status
  assert_equal 429, Http.get(\"https://api.x.io/long\").status
  assert_equal 503, Http.get(\"https://api.x.io/soon\").status
  assert_equal 3, Http.requests.size
end
",
    );
    let e = run_err("Http.get(\"https://api.x.io/a\", retry: 3)\n", Vec::new());
    assert_eq!(e.ty, "ArgumentError");
}

#[test]
fn a_retry_waits_what_the_server_asks() {
    let base = server(Arc::new(AtomicUsize::new(0)));
    let out = run(&"t = Time.now
r = Http.get(\"BASE/busy\")
puts r.status
puts (Time.now - t)
"
    .replace("BASE", &base));
    assert!(out.starts_with("200"), "{out}");
    assert!((0.9..10.0).contains(&seconds(&out)), "{out}");
}

#[test]
fn a_429_pauses_the_host_for_every_task() {
    let out = tests_pass(
        "limit net(\"api.x.io\"), 100.per_second
test \"the next call waits out the pause\" do
  mock_http \"GET https://api.x.io/*\", body: \"ok\"
  mock_http \"GET https://api.x.io/*\", status: 429, headers: {\"Retry-After\" => \"1\"}, times: 1
  assert_equal 429, Http.get(\"https://api.x.io/a\", retry: false).status
  t = Time.now
  assert_equal 200, Http.get(\"https://api.x.io/b\").status
  puts (Time.now - t)
end
",
    );
    assert!((0.9..10.0).contains(&seconds(&out)), "{out}");
}

// ── Declarations ─────────────────────────────────────────────────────

#[test]
fn rates_are_values() {
    let out = run("r = 5000.per_hour
puts r.count
puts r.period
puts 100.per(15.minutes).period
puts 10.per_second.count
");
    assert_eq!(out, "5000\n1h\n15min\n10\n");
}

#[test]
fn bad_declarations_are_refused() {
    for (src, ty, needle) in [
        ("limit net(\"https://api.x.io\"), 10.per_second\n", "ArgumentError", "expects a host"),
        ("limit net(\"api.x.io:443\"), 10.per_second\n", "ArgumentError", "expects a host"),
        ("limit net(\"api.x.io\"), 0.per_second\n", "ArgumentError", "at least one call"),
        ("limit net(\"api.x.io\"), 10\n", "TypeError", "expects a rate"),
        ("limit net(\"api.x.io\")\n", "ArgumentError", "a rate, `concurrent:`, or both"),
        ("limit net(\"api.x.io\"), 10.per_second, concurrent: 0\n", "ArgumentError", "positive count"),
        ("limit net(\"api.x.io\"), 10.per_second, burst: 3\n", "ArgumentError", "burst"),
        ("limit net(42), 10.per_second\n", "TypeError", "expects a host"),
        ("limit \"api.x.io\", 10.per_second\n", "ArgumentError", "expects a host first"),
        ("limit net(\"a.io\"), 1.per_second\nlimit net(\"A.io\"), 2.per_second\n", "ArgumentError", "already limited"),
        ("x = 10.per(3)\n", "ArgumentError", "expects a duration"),
    ] {
        let e = run_err(src, Vec::new());
        assert_eq!(e.ty, ty, "{src}: {}", e.message);
        assert!(e.message.contains(needle), "{src}: {}", e.message);
    }
}

#[test]
fn a_tests_limit_ends_with_it_and_the_others_start_full() {
    // without a refill, the second test's call would wait a minute
    let out = tests_pass(
        "limit net(\"api.x.io\"), 1.per_minute
test \"one call\" do
  mock_http \"https://*\", body: \"ok\"
  Http.get(\"https://api.x.io/a\")
  limit net(\"other.io\"), 1.per_minute
  Http.get(\"https://other.io/a\")
end
test \"one more call, and the test's own limit gone\" do
  mock_http \"https://*\", body: \"ok\"
  t = Time.now
  Http.get(\"https://api.x.io/b\")
  Http.get(\"https://other.io/b\")
  Http.get(\"https://other.io/c\")
  puts (Time.now - t)
end
",
    );
    assert!(seconds(&out) < 1.0, "{out}");
}

#[test]
fn waits_are_logged() {
    let run = run_mode(
        "limit net(\"api.x.io\"), 1.per_second
mock_http \"GET https://api.x.io/*\", body: \"ok\"
Http.get(\"https://api.x.io/a\")
Http.get(\"https://api.x.io/b\")
",
        Scripted::new([]),
        &[],
        &[],
        Mode { log: true, ..Mode::default() },
    );
    let out = run.ok();
    assert!(out.contains("[limit] net(\"api.x.io\"): 1 per second"), "{out}");
    assert!(out.contains("[limit] net(\"api.x.io\"): waited"), "{out}");
}

#[test]
fn jobs_and_tasks_share_the_bucket() {
    // the job's two calls empty the bucket: the test's own call waits
    let out = tests_pass(
        "limit net(\"api.x.io\"), 2.per_second
def sync(n: Int) -> Int uses net(\"api.x.io\")
  Http.get(\"https://api.x.io/a/#{n}\").status + Http.get(\"https://api.x.io/b/#{n}\").status
end
test \"a job, then the test\" do
  mock_http \"GET https://api.x.io/*\", body: \"ok\"
  t = Time.now
  enqueue(:sync, 1)
  Jobs.perform
  Http.get(\"https://api.x.io/c\")
  assert_equal 3, Http.requests.size
  puts (Time.now - t)
end
",
    );
    assert!((0.45..10.0).contains(&seconds(&out)), "{out}");
}
