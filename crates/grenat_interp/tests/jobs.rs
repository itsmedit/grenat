//! Jobs: queued in the database, performed, retried, and never untrusted.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, run_tests};

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

const APP: &str = "\
struct Note
  table :notes
  id: Int?
  text: String
end

migration \"001_notes\" do |db|
  db.migrate(\"CREATE TABLE notes (id #{db.primary_key}, text TEXT NOT NULL)\")
end

def remember(text: String, times: Int) uses db
  times.times { |i| Note.create(text: \"#{text} #{i}\") }
  enqueue(:done, text) if times > 1
end

def done(text: String) uses db
  Note.create(text: \"done: #{text}\")
end

def flaky(n: Int)
  raise \"boom #{n}\"
end
";

#[test]
fn jobs_are_queued_then_performed() {
    let results = results(&format!(
        "{APP}
test \"queued, then performed\" do
  enqueue(:remember, \"hi\", 2)
  assert_equal [[:remember, [\"hi\", 2]]], Jobs.enqueued
  assert_equal 0, Note.count
  assert_equal 2, Jobs.perform
  assert_equal [\"hi 0\", \"hi 1\", \"done: hi\"], Note.all.map {{ |n| n.text }}
  assert_equal [], Jobs.enqueued
end
test \"failures are retried, then given up\" do
  enqueue(:flaky, 7)
  Jobs.perform
  assert_equal [[\"flaky\", \"RuntimeError: boom 7\"]], Jobs.failed
end
test \"jobs run functions, with data\" do
  enqueue(:nowhere)
end
"
    ));
    assert_eq!(results[0].1, None);
    assert_eq!(results[1].1, None);
    assert_eq!(results[2].1.as_deref(), Some("NameError: `enqueue`: unknown function `nowhere`"));
}

#[test]
fn a_job_never_carries_an_untrusted_value() {
    let results = results(&format!(
        "{APP}
post \"/notes\" do |req|
  enqueue(:remember, req.json[\"text\"], 1)
end
test \"refused\" do
  request :post, \"/notes\", json: {{text: \"x\"}}
end
"
    ));
    assert!(
        results[0].1.as_deref().unwrap().starts_with("TaintError: an untrusted value reaches the job `remember`"),
        "{results:?}"
    );
}

const WEEKLY: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"

prompt digest(monday: String, repos: Array(String)) -> ~String using :fast
  user \"digest of #{monday}: #{repos.join(\", \")}\"
end

workflow weekly(monday: String, repos: Array(String) = [\"rust-lang/rust\"], tone: String = \"plain\") -> String
  step(:digest) { digest(monday, repos).trust! } + \" (#{tone})\"
end
";

#[test]
fn a_job_keeps_its_keyword_arguments() {
    let results = results(&format!(
        "{WEEKLY}
test \"keywords\" do
  mock :fast, replies: [\"d1\"]
  enqueue(:weekly, \"2026-09-28\", tone: \"warm\", in: 1.hour)
  assert_equal [[:weekly, [\"2026-09-28\", {{tone: \"warm\"}}]]], Jobs.enqueued
  assert_equal 1, Jobs.perform
  assert_equal [], Jobs.failed
  assert_equal \"d1 (warm)\", weekly(\"2026-09-28\", tone: \"warm\")
end
test \"keywords only\" do
  mock :fast, replies: [\"d2\"]
  enqueue(:weekly, monday: \"2026-10-05\")
  Jobs.perform
  assert_equal [], Jobs.failed
  assert_equal \"d2 (plain)\", weekly(monday: \"2026-10-05\")
end
test \"an untrusted keyword is refused\" do
  mock_http \"GET https://x.io/\", json: {{m: \"2026-10-05\"}}
  assert_raises TaintError {{ enqueue(:weekly, monday: Http.get(\"https://x.io/\").json[\"m\"]) }}
  assert_equal [], Jobs.enqueued
end
test \"an unknown keyword fails the job\" do
  enqueue(:weekly, mondy: \"2026-10-05\")
  Jobs.perform
  assert_equal [[\"weekly\", \"ArgumentError: missing argument `monday` for `weekly`\"]], Jobs.failed
end
"
    ));
    assert!(results.iter().all(|(_, e)| e.is_none()), "{results:?}");
}

#[test]
fn a_run_is_its_arguments_however_they_are_written() {
    let results = results(&format!(
        "{WEEKLY}
test \"positional, keyword, reordered: one run\" do
  mock :fast, replies: [\"d1\", \"d2\"]
  assert_equal \"d1 (plain)\", weekly(\"2026-09-28\", [\"a/b\"])
  assert_equal \"d1 (plain)\", weekly(monday: \"2026-09-28\", repos: [\"a/b\"])
  assert_equal \"d1 (plain)\", weekly(repos: [\"a/b\"], monday: \"2026-09-28\")
  assert_equal \"d1 (plain)\", weekly(\"2026-09-28\", repos: [\"a/b\"])
  # a keyword after a default: still one run however it is written
  assert_equal \"d2 (warm)\", weekly(\"2026-09-28\", tone: \"warm\")
  assert_equal \"d2 (warm)\", weekly(tone: \"warm\", monday: \"2026-09-28\")
end
"
    ));
    assert!(results.iter().all(|(_, e)| e.is_none()), "{results:?}");
}
