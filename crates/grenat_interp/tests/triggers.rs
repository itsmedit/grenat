//! Triggers: webhooks (signatures, tokens, responses) and schedules.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, run_tests};

fn tests(src: &str) -> Vec<(String, Option<String>)> {
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

fn all_pass(src: &str) {
    for (name, error) in tests(src) {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
}

#[test]
fn signed_webhooks_reach_their_handler_others_do_not() {
    all_pass(
        "\
on_webhook \"/github\", secret: \"s3cret\", signature: :github do |req|
  event = req.json.trust!
  \"opened ##{event[\"number\"]}\"
end
on_webhook \"/deploy\", token: \"t0k\" do |req|
  202
end
test \"signed as GitHub signs\" do
  r = deliver_webhook \"/github\", json: {action: \"opened\", number: 12}
  assert_equal 200, r[\"status\"]
  assert_equal \"opened #12\", r[\"body\"]
end
test \"a forged signature is refused\" do
  r = deliver_webhook \"/github\", json: {number: 1}, headers: {\"X-Hub-Signature-256\" => \"sha256=00\"}
  assert_equal 401, r[\"status\"]
end
test \"a token\" do
  assert_equal 202, (deliver_webhook \"/deploy\", body: \"go\")[\"status\"]
  r = deliver_webhook \"/deploy\", body: \"go\", headers: {\"Authorization\" => \"Bearer nope\"}
  assert_equal 401, r[\"status\"]
end
test \"unknown paths\" do
  assert_equal 404, (deliver_webhook \"/nope\", body: \"\")[\"status\"]
end
",
    );
}

#[test]
fn what_a_handler_returns_is_the_response() {
    all_pass(
        "\
on_webhook \"/nil\" do |req|
  nil
end
on_webhook \"/hash\" do |req|
  {ok: true, path: req.path}
end
test \"responses\" do
  assert_equal 204, (deliver_webhook \"/nil\", body: \"\")[\"status\"]
  r = deliver_webhook \"/hash\", body: \"\"
  assert_equal \"{\\\"ok\\\":true,\\\"path\\\":\\\"/hash\\\"}\", r[\"body\"]
end
",
    );
}

#[test]
fn what_a_webhook_carries_is_untrusted() {
    let results = tests(
        "\
on_webhook \"/cmd\" do |req|
  Shell.run([\"echo\", req.body])
end
test \"untrusted\" do
  deliver_webhook \"/cmd\", body: \"rm -rf /\"
end
",
    );
    assert!(results[0].1.as_deref().unwrap().starts_with("TaintError: an untrusted value reaches `Shell.run`"), "{results:?}");
}

#[test]
fn schedules_are_checked() {
    let e = run_err("every cron: \"61 * * * *\" do\nend\n", Vec::new());
    assert_eq!(e.message, "`61` is out of range 0-59");
    let e = run_err("every \"daily\" do\nend\n", Vec::new());
    assert!(e.message.starts_with("`every` expects a duration"), "{}", e.message);
    let e = run_err("on_webhook \"github\" do |r|\nend\n", Vec::new());
    assert_eq!(e.message, "a webhook path starts with `/`, got \"github\"");
    // declared, not run, by `grenat run`
    assert_eq!(run("every 0.01 do\n  puts \"tick\"\nend\nputs \"done\"\n"), "done\n");
}
