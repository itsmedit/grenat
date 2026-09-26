//! Secrets: credentials as `Secret` values — never shown, never sent to a
//! model, never journaled, queued or stored.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Response, run_main, run_tests};
use serde_json::json;

const CREDENTIALS: &str = "mock_credentials({\"github\" => {\"token\" => \"ghp_1\", \"port\" => 22}, \"empty\" => nil})\n";

fn results(src: &str) -> Vec<(String, Option<String>)> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    run_tests(&parsed.program, options)
        .unwrap()
        .into_iter()
        .map(|o| (o.name, o.error.map(|e| format!("{}: {}", e.ty, e.message))))
        .collect()
}

fn error_of(src: &str) -> String {
    let e = run_err(&format!("{CREDENTIALS}{src}"), Vec::new());
    format!("{}: {}", e.ty, e.message)
}

#[test]
fn a_secret_shows_as_secret_and_compares_to_text() {
    let out = run(&format!(
        "{CREDENTIALS}t = Credentials.fetch(:github, :token)
puts t
p t, [t], {{token: t}}
puts \"Bearer #{{t}}\"
puts \"x\" + t
p t == \"ghp_1\", t == \"nope\", \"ghp_1\" == t, t.to_s == t
p Credentials.fetch(:github, :port)
p Credentials.dig(:github, :missing), Credentials.dig(:empty)
"
    ));
    assert_eq!(out, "[secret]\n[secret]\n[[secret]]\n{token: [secret]}\n[secret]\n[secret]\ntrue\nfalse\ntrue\ntrue\n[secret]\nnil\nnil\n");
}

#[test]
fn missing_credentials_are_errors() {
    assert_eq!(error_of("Credentials.fetch(:github, :nope)"), "KeyError: no credential `github.nope` (grenat credentials edit)");
    assert_eq!(error_of("Credentials.fetch(:empty)"), "KeyError: the credential `empty` is empty");
    assert_eq!(error_of("Credentials.fetch(:github)"), "TypeError: `github` holds several credentials: fetch one of its keys");
    assert_eq!(error_of("Credentials.fetch()").split(':').next(), Some("ArgumentError"));
    let e = run_err("Credentials.fetch(:github, :token)", Vec::new());
    assert_eq!(e.ty, "CredentialsError", "{}", e.message);
}

#[test]
fn a_secret_offers_no_method() {
    assert_eq!(
        error_of("Credentials.fetch(:github, :token).size"),
        "SecretError: a secret has no method `size`: pass it where it serves (a header, a URL, a connection)"
    );
}

#[test]
fn a_secret_never_reaches_a_model() {
    let prompt = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt leak(t: Secret) -> ~String using :fast
  user \"Here: #{t}\"
end
";
    let e = run_err(&format!("{CREDENTIALS}{prompt}leak(Credentials.fetch(:github, :token))\n"), vec![Response::text_reply("no")]);
    assert_eq!((e.ty.as_str(), e.message.as_str()), ("SecretError", "a secret never reaches a model: keep it for headers, URLs and connections"));
    // a tool that answers with a secret: the model gets an error instead
    let agent = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
tool token -> String
  Credentials.fetch(:github, :token)
end
agent Helper
  model :fast
  tools token
  on Ask(q: String) -> ~String
    run q
  end
end
puts spawn(Helper).ask(Ask(q: \"the token?\")).trust!
";
    let replies = vec![Response::tool_call("t1", "token", json!({})), Response::tool_call("t2", "final_answer", json!({"value": "refused"}))];
    let run = run_full(&format!("{CREDENTIALS}{agent}"), replies, &[], &[]);
    let requests = run.requests.clone();
    assert_eq!(run.ok(), "refused\n");
    let second = requests[1].to_string();
    assert!(second.contains("SecretError") && !second.contains("ghp_1"), "{second}");
}

#[test]
fn a_secret_is_never_journaled_queued_or_stored() {
    let journaled = "workflow keep -> Int
  step(:a) { Credentials.fetch(:github, :token) }
  1
end
keep
";
    assert!(error_of(journaled).contains("a secret is never journaled nor queued"), "{}", error_of(journaled));
    let queued = "database \"sqlite::memory:\"\ndef use(t: Secret) = 1\nenqueue(:use, Credentials.fetch(:github, :token))\n";
    assert!(error_of(queued).contains("a secret is never journaled nor queued"), "{}", error_of(queued));
    let stored = "db = Db.connect(\"sqlite::memory:\")\ndb.execute(\"CREATE TABLE t (x TEXT)\")\ndb.execute(\"INSERT INTO t VALUES (?)\", [Credentials.fetch(:github, :token)])\n";
    assert_eq!(error_of(stored), "SecretError: a secret is never written to a database: it stays in the credentials");
}

#[test]
fn tests_give_their_own_credentials_one_test_at_a_time() {
    let src = "test \"with\" do
  mock_credentials({\"api\" => {\"key\" => \"k\"}})
  assert Credentials.fetch(:api, :key) == \"k\"
end
test \"without\" do
  assert_raises(CredentialsError) { Credentials.fetch(:api, :key) }
end
";
    for (name, error) in results(src) {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
}

#[test]
fn credentials_are_read_from_the_application() {
    let root = temp_dir("credentials-app");
    let location = grenat_config::credentials::Location::of(&root, None);
    location.create().unwrap();
    location.write("stripe:\n  key: sk_live_9\n").unwrap();
    let parsed = grenat_parser::parse("p Credentials.fetch(:stripe, :key) == \"sk_live_9\"\n");
    let buffer = Arc::new(Mutex::new(String::new()));
    let options = Options { output: Output::Capture(buffer.clone()), credentials_root: Some(root), ..Options::default() };
    run_main(&parsed.program, Vec::new(), options).unwrap();
    assert_eq!(buffer.lock().unwrap().as_str(), "true\n");
}
