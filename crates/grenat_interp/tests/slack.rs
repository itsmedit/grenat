//! Slack and Teams: webhooks they sign (`signature: :slack`, `:teams`,
//! Slack's `url_verification`), `Slack.post`, and approvals decided with
//! one click (`approvals_in_slack`) — in tests through `deliver_webhook`,
//! `mock_http` and `deliver_slack_click`, then under `grenat serve` against
//! a fake Slack: a question posted with its buttons, a click signed as
//! Slack signs it deciding the approval and resuming the job, the message
//! updated, and every refusal (a stale or replayed click, a forged one, a
//! user not allowed, a click on an approval already decided).

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use grenat_interp::{Options, Output};
use serde_json::{Value as Json, json};

// ── In tests: signed webhooks ──────────────────────────────────────────

#[test]
fn slack_signs_its_requests_and_its_url_verification_is_answered() {
    tests_pass(
        "on_webhook \"/slack/events\", signature: :slack, secret: \"8f742231b10e8888abcd99yyyzzz85a5\" do |req|
  event = req.json.trust!
  \"got #{event[\"type\"]}\"
end
test \"signed as Slack signs\" do
  r = deliver_webhook \"/slack/events\", json: {type: \"event_callback\"}
  assert_equal 200, r[\"status\"]
  assert_equal \"got event_callback\", r[\"body\"]
end
test \"the same body twice: each delivery signed anew\" do
  assert_equal 200, (deliver_webhook \"/slack/events\", body: \"{}\")[\"status\"]
  assert_equal 200, (deliver_webhook \"/slack/events\", body: \"{}\")[\"status\"]
end
test \"a forged signature\" do
  now = Time.now.to_i.to_s
  r = deliver_webhook \"/slack/events\", body: \"{}\", headers: {\"X-Slack-Signature\" => \"v0=00\", \"X-Slack-Request-Timestamp\" => now}
  assert_equal 401, r[\"status\"]
end
test \"the challenge is answered once verified, without the handler\" do
  r = deliver_webhook \"/slack/events\", json: {token: \"x\", challenge: \"3eZbrw1aBm2rZgRNFdxV\", type: \"url_verification\"}
  assert_equal 200, r[\"status\"]
  assert_equal \"3eZbrw1aBm2rZgRNFdxV\", r[\"body\"]
end
",
    );
}

#[test]
fn teams_signs_with_its_security_token_decoded() {
    tests_pass(
        "on_webhook \"/teams\", signature: :teams, secret: \"vqF0En+Z0ucuRTM/01o2GuhMH3hKKk/N2bOmlM31zaA=\" do |req|
  {type: \"message\", text: \"On it.\"}
end
test \"signed as Teams signs\" do
  r = deliver_webhook \"/teams\", json: {type: \"message\", text: \"<at>Ops</at> status\"}
  assert_equal 200, r[\"status\"]
  assert_equal({\"type\" => \"message\", \"text\" => \"On it.\"}, Json.parse(r[\"body\"]))
end
test \"another key's signature\" do
  r = deliver_webhook \"/teams\", body: \"{}\", headers: {\"Authorization\" => \"HMAC 03TCao0i55H1eVKUusZOTZRjtvYTs+mO41mPL+R1e1U=\"}
  assert_equal 401, r[\"status\"]
end
",
    );
}

#[test]
fn a_webhooks_proof_is_checked_when_declared() {
    let e = run_err("on_webhook \"/x\", signature: :gitlab, secret: \"s\" do |r|\nend\n", Vec::new());
    assert!(e.message.contains("knows no `signature: :gitlab`"), "{}", e.message);
    let e = run_err("on_webhook \"/x\", signature: :teams, secret: \"not base64!\" do |r|\nend\n", Vec::new());
    assert!(e.message.contains("security token"), "{}", e.message);
    let e = run_err("on_webhook \"/x\", signature: :slack do |r|\nend\n", Vec::new());
    assert_eq!(e.ty, "ArgumentError");
    let e = run_err("on_webhook \"/x\", secret: \"s\", signature: :slack, every: 1 do |r|\nend\n", Vec::new());
    assert!(e.message.contains("no option `every:`"), "{}", e.message);
}

// ── In tests: Slack.post ───────────────────────────────────────────────

#[test]
fn slack_post_calls_chat_post_message_with_the_token() {
    tests_pass(
        "test \"posted\" do
  mock_http \"POST https://slack.com/api/chat.postMessage\", json: {ok: true, channel: \"C0OPS\", ts: \"1700000000.000200\"}
  ts = Slack.post(\"#ops\", \"Deploy done\", token: \"xoxb-1\")
  assert ts.tainted?
  assert_equal \"1700000000.000200\", ts.trust!
  sent = Http.requests.last
  assert_equal \"https://slack.com/api/chat.postMessage\", sent[\"url\"]
  assert_equal \"#ops\", sent[\"json\"][\"channel\"]
  assert_equal \"Deploy done\", sent[\"json\"][\"text\"]
end
test \"the bot token from the credentials\" do
  mock_credentials({\"slack\" => {\"bot_token\" => \"xoxb-2\"}})
  mock_http \"POST https://slack.com/api/chat.postMessage\", json: {ok: true, ts: \"1.2\"}
  Slack.post(\"C0OPS\", \"hi\")
  assert_equal 1, Http.requests.size
end
test \"Slack refuses: a SlackError naming the method and the code\" do
  mock_http \"POST https://slack.com/api/chat.postMessage\", json: {ok: false, error: \"channel_not_found\"}
  begin
    Slack.post(\"#nope\", \"hi\", token: \"xoxb-1\")
    assert false
  rescue SlackError => e
    assert_equal \"chat.postMessage: channel_not_found\", e.message
  end
end
",
    );
}

#[test]
fn slack_post_posts_nothing_untrusted_and_never_shows_its_token() {
    let (results, _) = test_outcomes(
        "on_webhook \"/in\" do |req|
  Slack.post(\"#ops\", req.body, token: \"xoxb-1\")
end
test \"a payload posted unchecked\" do
  r = deliver_webhook \"/in\", body: \"<!channel> pwned\"
  assert_equal 500, r[\"status\"]
end
test \"a secret is never posted\" do
  Slack.post(\"#ops\", Credentials.fetch(:slack, :bot_token), token: \"xoxb-1\")
end
test \"refused by Slack's transport\" do
  Slack.post(\"#ops\", \"hi\", token: \"xoxb-very-secret\")
end
",
    );
    assert_eq!(results[1].1.as_deref().map(|e| e.starts_with("SecretError")), Some(true), "{results:?}");
    let refused = results[2].1.as_deref().unwrap();
    assert!(refused.starts_with("SlackError: chat.postMessage:") && !refused.contains("xoxb"), "{refused}");
    let e = run_err(&format!("{SUMMARY}s = summarize(\"x\")\nSlack.post(\"#ops\", s.title)\n"), vec![summary_reply()]);
    assert_eq!(e.ty, "TaintError");
}

// ── In tests: approvals decided in Slack ───────────────────────────────

const APPROVALS: &str = "\
approvals_in_slack channel: \"#approvals\", token: \"xoxb-test\", signing_secret: \"shh\", allowed: [\"U123\", \"U456\"]

workflow deploy(version: String) uses human, net
  step(:ask) { approve! \"Deploy #{version}?\" }
  step(:done) { puts \"deployed #{version}\" }
end
";

#[test]
fn a_click_decides_the_approval_and_updates_its_message() {
    let output = tests_pass(&format!(
        "{APPROVALS}
test \"approved in Slack\" do
  mock_http \"POST https://slack.com/api/*\", json: {{ok: true, channel: \"C0TEST\", ts: \"1700000000.000100\"}}
  enqueue(:deploy, \"v2\")
  Jobs.perform
  asked = Http.requests.last
  assert_equal \"https://slack.com/api/chat.postMessage\", asked[\"url\"]
  assert_equal \"#approvals\", asked[\"json\"][\"channel\"]
  id = Approvals.pending.last[\"id\"]
  buttons = asked[\"json\"][\"blocks\"][2][\"elements\"]
  assert_equal [\"grenat_approve\", \"grenat_deny\"], buttons.map {{ |b| b[\"action_id\"] }}
  assert_equal id.to_s, buttons.first[\"value\"]
  r = deliver_slack_click(id, action: :approve, user: \"U123\")
  assert_equal({{\"status\" => 200, \"outcome\" => \"approved\"}}, r)
  updated = Http.requests.last
  assert_equal \"https://slack.com/api/chat.update\", updated[\"url\"]
  assert_equal \"1700000000.000100\", updated[\"json\"][\"ts\"]
  assert updated[\"json\"][\"text\"].start_with?(\"Approved by <@U123>\")
  assert_equal 2, updated[\"json\"][\"blocks\"].size
  Jobs.perform
  assert_equal [], Approvals.pending.select {{ |a| a[\"id\"] == id }}
end
test \"denied, then a second click changes nothing\" do
  mock_http \"POST https://slack.com/api/*\", json: {{ok: true, ts: \"1700000000.000100\"}}
  enqueue(:deploy, \"v3\")
  Jobs.perform
  id = Approvals.pending.last[\"id\"]
  assert_equal \"denied\", deliver_slack_click(id, action: :deny, user: \"U456\")[\"outcome\"]
  again = deliver_slack_click(id, action: :approve, user: \"U123\")
  assert_equal({{\"status\" => 200, \"outcome\" => \"already_decided\"}}, again)
  assert Http.requests.last[\"json\"][\"text\"].start_with?(\"Denied by <@U456>\")
  Jobs.perform
  assert Jobs.failed.first[1].start_with?(\"ApprovalDenied\")
end
test \"a user not allowed decides nothing and is told privately\" do
  mock_http \"POST https://slack.com/api/*\", json: {{ok: true, ts: \"1700000000.000100\"}}
  enqueue(:deploy, \"v4\")
  Jobs.perform
  id = Approvals.pending.last[\"id\"]
  assert_equal \"not_allowed\", deliver_slack_click(id, action: :approve, user: \"U999\")[\"outcome\"]
  told = Http.requests.last
  assert_equal \"https://slack.com/api/chat.postEphemeral\", told[\"url\"]
  assert_equal \"U999\", told[\"json\"][\"user\"]
  assert_equal [id], Approvals.pending.map {{ |a| a[\"id\"] }}.select {{ |a| a == id }}
end
test \"an approval that does not exist\" do
  mock_http \"POST https://slack.com/api/*\", json: {{ok: true}}
  assert_equal \"unknown\", deliver_slack_click(4242, action: :approve, user: \"U123\")[\"outcome\"]
end
test \"Slack down: the job still waits, decided in the console or by the program\" do
  mock_http \"POST https://slack.com/api/*\", status: 503
  enqueue(:deploy, \"v5\")
  Jobs.perform
  waiting = Approvals.pending.select {{ |a| a[\"message\"] == \"Deploy v5?\" }}
  assert_equal 1, waiting.size
  Approvals.approve(waiting.first[\"id\"])
  Jobs.perform
  assert_equal [], Approvals.pending.select {{ |a| a[\"message\"] == \"Deploy v5?\" }}
end
"
    ));
    assert!(output.contains("deployed v2\n") && output.contains("deployed v5\n"), "{output}");
    assert!(!output.contains("deployed v3") && !output.contains("deployed v4"), "{output}");
}

#[test]
fn the_token_of_approvals_in_slack_is_a_secret_in_what_a_test_sees() {
    let output = tests_pass(&format!(
        "{APPROVALS}
test \"the token\" do
  mock_http \"POST https://slack.com/api/*\", json: {{ok: true, ts: \"1.2\"}}
  enqueue(:deploy, \"v2\")
  Jobs.perform
  sent = Http.requests.last
  assert_equal \"Bearer xoxb-test\", sent[\"headers\"][\"Authorization\"]
  p sent[\"headers\"]
end
"
    ));
    assert!(output.contains("\"Authorization\" => [secret]") && !output.contains("xoxb-test"), "{output}");
}

#[test]
fn approvals_in_slack_is_declared_once_with_what_it_needs() {
    let declared = |options: &str| run_err(&format!("approvals_in_slack {options}\n"), Vec::new());
    let e = declared("channel: \"#a\", token: \"x\"");
    assert!(e.message.contains("expects `signing_secret:`"), "{}", e.message);
    let e = declared("channel: \"#a\", token: \"x\", signing_secret: \"s\", allowed: [\"<!channel>\"]");
    assert!(e.message.contains("Slack user IDs"), "{}", e.message);
    let e = declared("channel: \"#a\", token: \"x\", signing_secret: \"s\", path: \"slack\"");
    assert_eq!(e.ty, "ArgumentError");
    let e = declared("channel: \"#a\", token: \"x\", signing_secret: \"s\", room: \"r\"");
    assert!(e.message.contains("option `room:"), "{}", e.message);
    let once = "approvals_in_slack channel: \"#a\", token: \"x\", signing_secret: \"s\"\n";
    let e = run_err(&format!("{once}{once}"), Vec::new());
    assert!(e.message.contains("declared once"), "{}", e.message);
    let e = run_err(&format!("{once}on_webhook \"/slack/interactivity\" do |r|\nend\n"), Vec::new());
    assert!(e.message.contains("route of `approvals_in_slack`"), "{}", e.message);
    let e = run_err(
        &format!(
            "{SUMMARY}s = summarize(\"x\")\napprovals_in_slack channel: s.title, token: \"x\", signing_secret: \"s\"\n"
        ),
        vec![summary_reply()],
    );
    assert_eq!(e.ty, "TaintError");
    // a refused secret is never shown
    let e = run_err(
        "mock_credentials({\"slack\" => {\"t\" => \"xoxb-hidden\"}})\napprovals_in_slack channel: \"#a\", token: \"x\", signing_secret: \"s\", allowed: Credentials.fetch(:slack, :t)\n",
        Vec::new(),
    );
    assert!(!e.message.contains("xoxb-hidden"), "{}", e.message);
    // `deliver_slack_click` is a test double
    let e = run_err(&format!("{once}deliver_slack_click(1, action: :approve, user: \"U1\")\n"), Vec::new());
    assert!(e.message.contains("only works in a test"), "{}", e.message);
    // declared, nothing posted
    assert_eq!(run(&format!("{once}puts \"ok\"\n")), "ok\n");
}

// ── Served, against a fake Slack ───────────────────────────────────────

/// A call the fake Slack received: the method, the `Authorization` header,
/// the JSON body.
#[derive(Debug, Clone)]
struct Call {
    method: String,
    auth: String,
    body: Json,
}

/// Slack's Web API, faked: every method answers `ok` with a message's `ts`.
struct FakeSlack {
    base: String,
    calls: Arc<Mutex<Vec<Call>>>,
}

impl FakeSlack {
    fn start() -> FakeSlack {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/api", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (path, headers, body) = read_request(&stream);
                let method = path.trim_start_matches("/api/").to_string();
                let auth = header(&headers, "authorization");
                let body: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
                seen.lock().unwrap().push(Call { method, auth, body });
                let answer = json!({"ok": true, "channel": "C0APPROVALS", "ts": "1700000000.000100"}).to_string();
                write_response(stream, 200, &answer);
            }
        });
        FakeSlack { base, calls }
    }

    fn calls(&self, method: &str) -> Vec<Call> {
        self.calls.lock().unwrap().iter().filter(|c| c.method == method).cloned().collect()
    }
}

fn read_request(stream: &TcpStream) -> (String, Vec<(String, String)>, Vec<u8>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    let path = first.split_whitespace().nth(1).unwrap_or_default().to_string();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
    let length = header(&headers, "content-length").parse().unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    (path, headers, body)
}

fn header(headers: &[(String, String)], name: &str) -> String {
    headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_default()
}

fn write_response(mut stream: TcpStream, status: u16, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Sends a request to the served program: its status and body.
fn send(address: &str, path: &str, headers: &[(&str, String)], body: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut head =
        format!("POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let status = answer.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = answer.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// The headers Slack sends with `body`, signed with `secret` at `timestamp`.
fn signed(secret: &str, timestamp: i64, body: &[u8]) -> Vec<(&'static str, String)> {
    vec![
        ("X-Slack-Request-Timestamp", timestamp.to_string()),
        ("X-Slack-Signature", grenat_serve::signature::slack(secret, &timestamp.to_string(), body)),
        ("Content-Type", "application/x-www-form-urlencoded".into()),
    ]
}

/// The body of a click on approval `id`'s button, as Slack sends it.
fn click(id: &str, action: &str, user: &str, trigger: u32) -> Vec<u8> {
    let payload = json!({
        "type": "block_actions",
        "user": {"id": user, "username": "jtorrance", "team_id": "T9TK3CUKW"},
        "api_app_id": "AABA1ABCD",
        "container": {"type": "message", "message_ts": "1700000000.000100", "channel_id": "C0APPROVALS"},
        "trigger_id": format!("{trigger}.333649436676.d8c1bb837935619ccad0f624c448ffb3"),
        "channel": {"id": "C0APPROVALS", "name": "approvals"},
        "message": {"bot_id": "BAH5CA16Z", "ts": "1700000000.000100"},
        "response_url": "https://hooks.slack.com/actions/AABA1ABCD/1/x",
        "actions": [{"action_id": action, "block_id": "grenat_approval", "value": id, "type": "button", "action_ts": "1700000001.000000"}],
    });
    let encoded: String = payload
        .to_string()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("payload={encoded}").into_bytes()
}

/// Serves `src` with Slack's Web API at `slack`: the address it listens
/// on, and what it prints and logs.
fn serve(src: &str, slack: &FakeSlack) -> (String, Arc<Mutex<String>>) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let program: &'static grenat_ast::Program = Box::leak(Box::new(parsed.program));
    let buffer = Arc::new(Mutex::new(String::new()));
    let options = Options {
        output: Output::Capture(buffer.clone()),
        journal: Some(temp_dir("journal")),
        log: true,
        slack_api: Some(slack.base.clone()),
        ..Options::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        grenat_interp::serve(program, options, "127.0.0.1:0", move |address| tx.send(address.to_string()).unwrap())
    });
    let address = rx.recv_timeout(Duration::from_secs(20)).expect("listening");
    (address, buffer)
}

/// Waits (20 s at most) until `done` holds.
fn wait_until(what: &str, output: &Arc<Mutex<String>>, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(20), "timed out waiting: {what}\n{}", output.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn database() -> (PathBuf, String) {
    let path = temp_dir("slack").join("app.db");
    let line = format!("database \"sqlite://{}\"\n", path.display());
    (path, line)
}

fn approval(path: &Path, id: i64) -> grenat_ops::approvals::Approval {
    let mut connection = grenat_db::connect(&format!("sqlite://{}", path.display())).unwrap();
    grenat_ops::approvals::find(connection.as_mut(), id).unwrap().unwrap()
}

fn slack_events(path: &Path) -> Vec<(String, String)> {
    let mut connection = grenat_db::connect(&format!("sqlite://{}", path.display())).unwrap();
    let events = grenat_ops::events::latest(connection.as_mut(), false, 100).unwrap();
    events.into_iter().filter(|e| e.source == "slack").map(|e| (e.subject, e.message)).collect()
}

const SECRET: &str = "8f742231b10e8888abcd99yyyzzz85a5";

#[test]
fn served_a_click_in_slack_decides_and_the_job_resumes() {
    let slack = FakeSlack::start();
    let (db, database) = database();
    let src = format!(
        "{database}approvals_in_slack channel: \"#approvals\", token: \"xoxb-served\", signing_secret: \"{SECRET}\", allowed: [\"U123\"]

workflow deploy(version: String) uses human, net
  step(:ask) {{ approve! \"Deploy #{{version}} <!channel>?\" }}
  step(:done) {{ puts \"deployed #{{version}}\" }}
end

enqueue(:deploy, \"v2\")
"
    );
    let (address, output) = serve(&src, &slack);
    wait_until("the question posted", &output, || !slack.calls("chat.postMessage").is_empty());
    let asked = &slack.calls("chat.postMessage")[0];
    assert_eq!(asked.auth, "Bearer xoxb-served");
    assert_eq!(asked.body["channel"], "#approvals");
    // what the question quotes mentions no one
    assert!(asked.body["text"].as_str().unwrap().contains("&lt;!channel&gt;"), "{}", asked.body);
    let id = asked.body["blocks"][2]["elements"][0]["value"].as_str().unwrap().to_string();
    let path = "/slack/interactivity";

    // forged, stale, unsigned: refused before anything is read
    let body = click(&id, "grenat_approve", "U123", 1);
    assert_eq!(send(&address, path, &signed("wrong", now(), &body), &body).0, 401);
    assert_eq!(send(&address, path, &signed(SECRET, now() - 301, &body), &body).0, 401);
    assert_eq!(send(&address, path, &[], &body).0, 401);

    // a user not allowed: told privately, the approval still waits
    let stranger = click(&id, "grenat_approve", "U999", 2);
    assert_eq!(send(&address, path, &signed(SECRET, now(), &stranger), &stranger).0, 200);
    assert_eq!(slack.calls("chat.postEphemeral").len(), 1);
    assert_eq!(slack.calls("chat.postEphemeral")[0].body["user"], "U999");
    let waiting = approval(&db, id.parse().unwrap());
    assert_eq!(waiting.decision, grenat_ops::approvals::Decision::Pending);

    // allowed: decided, recorded with who decided, the message updated
    let sent = now();
    let (status, _) = send(&address, path, &signed(SECRET, sent, &body), &body);
    assert_eq!(status, 200);
    let decided = approval(&db, id.parse().unwrap());
    assert_eq!(decided.decision, grenat_ops::approvals::Decision::Approved);
    assert_eq!(decided.decided_by.as_deref(), Some("slack:U123"));
    let updated = &slack.calls("chat.update")[0];
    assert_eq!(
        (updated.body["channel"].as_str(), updated.body["ts"].as_str()),
        (Some("C0APPROVALS"), Some("1700000000.000100"))
    );
    assert!(updated.body["text"].as_str().unwrap().starts_with("Approved by <@U123>"), "{}", updated.body);
    assert!(updated.body["blocks"].as_array().unwrap().iter().all(|b| b["type"] != "actions"));
    wait_until("the job resumed", &output, || output.lock().unwrap().contains("deployed v2\n"));

    // the same click sent again: a replay
    assert_eq!(send(&address, path, &signed(SECRET, sent, &body), &body).0, 401);
    // clicked again, signed anew: already decided, nothing changes
    let deny = click(&id, "grenat_deny", "U123", 3);
    assert_eq!(send(&address, path, &signed(SECRET, now(), &deny), &deny).0, 200);
    assert_eq!(approval(&db, id.parse().unwrap()).decision, grenat_ops::approvals::Decision::Approved);
    assert_eq!(slack.calls("chat.update").len(), 2);
    assert!(slack.calls("chat.update")[1].body["text"].as_str().unwrap().starts_with("Approved by <@U123>"));
    assert_eq!(output.lock().unwrap().matches("deployed v2").count(), 1);

    // the refusal is an event; neither the token nor the secret shows anywhere
    let events = slack_events(&db);
    assert!(events.iter().any(|(_, m)| m.contains("U999 may not decide")), "{events:?}");
    let log = output.lock().unwrap().clone();
    assert!(!log.contains("xoxb-served") && !log.contains(SECRET), "{log}");
    assert!(log.contains(&format!("[approval] {id} approved by slack:U123")), "{log}");
}

#[test]
fn served_slack_events_and_teams_messages() {
    let slack = FakeSlack::start();
    let teams_token = "vqF0En+Z0ucuRTM/01o2GuhMH3hKKk/N2bOmlM31zaA=";
    let src = format!(
        "on_webhook \"/slack/events\", signature: :slack, secret: \"{SECRET}\" do |req|
  puts \"event #{{req.json.trust![\"event\"][\"type\"]}}\"
  nil
end
on_webhook \"/teams\", signature: :teams, secret: \"{teams_token}\" do |req|
  {{type: \"message\", text: \"On it.\"}}
end
"
    );
    let (address, output) = serve(&src, &slack);
    // Slack's url_verification, answered once verified
    let challenge = br#"{"token":"Jhj5dZrVaK7ZwHHjRyZWjbDl","challenge":"3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P","type":"url_verification"}"#;
    let answer = send(&address, "/slack/events", &signed(SECRET, now(), challenge), challenge);
    assert_eq!(answer, (200, "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P".to_string()));
    assert_eq!(send(&address, "/slack/events", &signed("wrong", now(), challenge), challenge).0, 401);
    // an event
    let event = br#"{"type":"event_callback","event":{"type":"app_mention","text":"<@U0BOT> status"}}"#;
    let sent = now();
    assert_eq!(send(&address, "/slack/events", &signed(SECRET, sent, event), event).0, 204);
    wait_until("the event handled", &output, || output.lock().unwrap().contains("event app_mention\n"));
    assert_eq!(send(&address, "/slack/events", &signed(SECRET, sent, event), event).0, 401, "a replay");
    assert_eq!(send(&address, "/slack/events", &signed(SECRET, now() + 400, event), event).0, 401, "from the future");
    // Teams
    let message = br#"{"type":"message","text":"<at>Ops</at> status"}"#;
    let key = grenat_serve::signature::teams_key(teams_token).unwrap();
    let authorization = grenat_serve::signature::teams(&key, message);
    let (status, body) = send(&address, "/teams", &[("Authorization", authorization.clone())], message);
    let body: Json = serde_json::from_str(&body).unwrap();
    assert_eq!((status, body), (200, json!({"type": "message", "text": "On it."})));
    assert_eq!(send(&address, "/teams", &[("Authorization", authorization)], b"{}").0, 401);
    assert_eq!(send(&address, "/teams", &[], message).0, 401);
    let log = output.lock().unwrap().clone();
    assert!(log.contains("refused (replayed)") && log.contains("refused (bad signature)"), "{log}");
}
