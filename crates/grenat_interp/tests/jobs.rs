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
    assert!(results[0].1.as_deref().unwrap().starts_with("TaintError: an untrusted value reaches the job `remember`"), "{results:?}");
}
