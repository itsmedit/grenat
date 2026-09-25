//! `Http`: real requests to a local server, capabilities, taint, `mock_http`.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Response, run_tests};
use serde_json::json;

/// A local HTTP server; returns its base URL (`http://127.0.0.1:port`).
fn server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut parts = first.split_whitespace();
                let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                let (mut length, mut auth) = (0, String::new());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let (name, value) = line.split_once(':').unwrap();
                    match name.to_ascii_lowercase().as_str() {
                        "content-length" => length = value.trim().parse().unwrap(),
                        "authorization" => auth = value.trim().to_string(),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let (status, content) = match path.as_str() {
                    "/repo" => (200, json!({"name": "grenat", "stars": 42}).to_string()),
                    "/echo" => (200, json!({"method": method, "body": body, "auth": auth}).to_string()),
                    "/slow" => {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        (200, "late".to_string())
                    }
                    _ => (404, "no such page".to_string()),
                };
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nX-Rate: 99\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{content}",
                    content.len()
                );
            });
        }
    });
    base
}

fn run_http(src: &str) -> String {
    run(&src.replace("BASE", &server()))
}

#[test]
fn get_returns_status_headers_and_body() {
    let out = run_http(
        "\
res = Http.get(\"BASE/repo\")
p res.status, res.ok?
p res.json[\"stars\"]
p res.headers[\"x-rate\"]
missing = Http.get(\"BASE/nowhere\")
p missing.status, missing.ok?, missing.body
",
    );
    assert_eq!(out, "200\ntrue\n~42\n~\"99\"\n404\nfalse\n~\"no such page\"\n");
}

#[test]
fn requests_send_json_bodies_and_headers() {
    let out = run_http(
        "\
res = Http.post(\"BASE/echo\", json: {title: \"Bug\", labels: [\"a\"]}, headers: {\"Authorization\" => \"Bearer t0k\"})
echo = res.json.trust!
p echo[\"method\"], Json.parse(echo[\"body\"]), echo[\"auth\"]
p Http.put(\"BASE/echo\", body: \"raw\").json.trust![\"body\"]
p Http.patch(\"BASE/echo\").json.trust![\"method\"], Http.delete(\"BASE/echo\").json.trust![\"method\"]
",
    );
    assert_eq!(
        out,
        "\"POST\"\n{\"labels\" => [\"a\"], \"title\" => \"Bug\"}\n\"Bearer t0k\"\n\"raw\"\n\"PATCH\"\n\"DELETE\"\n"
    );
}

#[test]
fn network_failures_and_timeouts_are_http_errors() {
    let base = server();
    let e = run_err(&format!("Http.get(\"{base}/slow\", timeout: 0.3)\n"), Vec::new());
    assert_eq!(e.ty, "HttpError");
    assert!(e.message.starts_with(&format!("GET {base}/slow: ")), "{}", e.message);
    let e = run_err("Http.get(\"http://127.0.0.1:1/x\")\n", Vec::new());
    assert_eq!(e.ty, "HttpError");
    let e = run_err("Http.get(\"ftp://x\")\n", Vec::new());
    assert_eq!((e.ty.as_str(), e.message.as_str()), ("ArgumentError", "`Http.get` expects an http(s) URL, got \"ftp://x\""));
    let e = run_err("Http.get(\"http://x\", retries: 3)\n", Vec::new());
    assert_eq!(e.message, "invalid `Http` option `retries: 3`");
}

#[test]
fn hosts_are_capabilities() {
    let src = "\
def github(url: String) -> Int uses net(\"api.github.com\")
  Http.get(url).status
end
def anywhere(url: String) -> Int uses net
  Http.get(url).status
end
p anywhere(\"BASE/repo\")
github(\"BASE/repo\")
";
    let src = src.replace("BASE", &server());
    let e = run_err(&src, Vec::new());
    assert_eq!(e.ty, "CapabilityError");
    assert!(e.message.contains("is not allowed by `github` (uses net(\"api.github.com\"))"), "{}", e.message);
}

#[test]
fn nothing_untrusted_goes_out_and_what_comes_back_is_untrusted() {
    let base = server();
    // a model's answer, unchecked, cannot be sent
    let src = format!("{SUMMARY}s = summarize(\"x\")\nHttp.post(\"{base}/echo\", json: {{title: s.title}})\n");
    let e = run_err(&src, vec![summary_reply()]);
    assert_eq!(e.ty, "TaintError");
    assert!(e.message.starts_with("an untrusted value reaches `Http.post`"), "{}", e.message);
    // nor can a response be sent on as it is
    let e = run_err(&format!("r = Http.get(\"{base}/repo\")\nHttp.post(\"{base}/echo\", body: r.body)\n"), Vec::new());
    assert_eq!(e.ty, "TaintError");
    // once checked, it can
    let out = run(&format!(
        "r = Http.get(\"{base}/repo\")\nstars = r.json[\"stars\"].check {{ |n| n > 0 }}?\np Http.post(\"{base}/echo\", json: {{stars: stars}}).status\n"
    ));
    assert_eq!(out, "200\n");
    // parsing untrusted text gives untrusted data
    assert_eq!(run(&format!("p Json.parse(Http.get(\"{base}/repo\").body)[\"name\"]\n")), "~\"grenat\"\n");
}

fn tests_output(src: &str) -> Vec<(String, Option<String>)> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        dir: Some(temp_dir("http")),
        ..Options::default()
    };
    run_tests(&parsed.program, options)
        .unwrap()
        .into_iter()
        .map(|o| (o.name, o.error.map(|e| format!("{}: {}", e.ty, e.message))))
        .collect()
}

#[test]
fn tests_stub_the_network_and_never_reach_it() {
    let results = tests_output(
        "\
test \"stubbed\" do
  mock_http \"GET https://api.github.com/repos/acme/app\", json: {stars: 7}
  mock_http \"https://api.github.com/search/*\", status: 503, body: \"down\"
  assert_equal 7, Http.get(\"https://api.github.com/repos/acme/app\").json[\"stars\"].trust!
  assert_equal 503, Http.get(\"https://api.github.com/search/code?q=x\").status
  assert_equal 503, Http.post(\"https://api.github.com/search/x\").status
end
test \"method matters\" do
  mock_http \"GET https://api.github.com/repos/acme/app\", json: {}
  Http.post(\"https://api.github.com/repos/acme/app\")
end
test \"stubs do not outlive their test\" do
  Http.get(\"https://api.github.com/repos/acme/app\")
end
",
    );
    assert_eq!(results[0].1, None);
    assert_eq!(
        results[1].1.as_deref(),
        Some("HttpError: no network in tests: `POST https://api.github.com/repos/acme/app` is not stubbed with `mock_http`")
    );
    assert!(results[2].1.as_deref().unwrap().starts_with("HttpError: no network in tests"));
}

#[test]
fn a_model_and_the_network_together() {
    // the model's answer, checked, goes to a web service
    let base = server();
    let src = format!(
        "{SUMMARY}def main uses llm, net\n  s = summarize(\"x\").check {{ |s| s.title.size < 40 }}?\n  p Http.post(\"{base}/echo\", json: {{title: s.title}}).json.trust![\"body\"]\nend\n"
    );
    let out = run_with(&src, vec![summary_reply()], &[]).ok();
    assert_eq!(out, "\"{\\\"title\\\":\\\"Cat\\\"}\"\n");
    let _ = Response::text_reply("");
}
