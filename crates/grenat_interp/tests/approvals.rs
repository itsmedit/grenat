//! Approvals that wait: a job asks, waits, and resumes where it stopped.

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
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"

struct Post
  table :posts
  id: Int?
  title: String
end

migration \"001_posts\" do |db|
  db.migrate(\"CREATE TABLE posts (id #{db.primary_key}, title TEXT NOT NULL)\")
end

prompt headline(topic: String) -> ~String using :fast
  user \"A headline about #{topic}\"
end

workflow publish(topic: String) uses llm, db, human
  title = step(:draft) { headline(topic).trust! }
  step(:review) { approve! \"Publish “#{title}”?\" }
  step(:publish) { Post.create(title:).id }
end
";

#[test]
fn a_job_waits_for_a_human_and_resumes_without_asking_the_model_again() {
    let results = results(&format!(
        "{APP}
test \"approved\" do
  mock :fast, replies: [\"Rust 2.0 is out\"]
  enqueue(:publish, \"rust\")
  Jobs.perform
  pending = Approvals.pending
  assert_equal 1, pending.size
  assert_equal \"Publish “Rust 2.0 is out”?\", pending.first[\"message\"]
  assert_equal 0, Post.count
  assert_equal [], Jobs.enqueued
  Approvals.approve(pending.first[\"id\"])
  Jobs.perform
  assert_equal [\"Rust 2.0 is out\"], Post.all.map {{ |p| p.title }}
  assert_equal [], Approvals.pending
end
test \"denied\" do
  mock :fast, replies: [\"Draft\"]
  enqueue(:publish, \"x\")
  Jobs.perform
  Approvals.deny(Approvals.pending.first[\"id\"])
  Jobs.perform
  assert_equal 0, Post.count
  assert Jobs.failed.first[1].start_with?(\"ApprovalDenied\")
end
test \"a decision is taken once\" do
  mock :fast, replies: [\"Draft\"]
  enqueue(:publish, \"x\")
  Jobs.perform
  id = Approvals.pending.first[\"id\"]
  Approvals.approve(id)
  Approvals.approve(id)
end
"
    ));
    assert_eq!(results[0].1, None);
    assert_eq!(results[1].1, None);
    assert!(results[2].1.as_deref().unwrap().ends_with("no pending approval 1"), "{results:?}");
}

#[test]
fn every_question_of_a_run_is_asked_in_turn() {
    let results = results(&format!(
        "{APP}
workflow two uses db, human
  step(:first) {{ approve! \"first?\" }}
  step(:one) {{ Post.create(title: \"one\").id }}
  step(:second) {{ approve! \"second?\" }}
  step(:two) {{ Post.create(title: \"two\").id }}
end
test \"two questions\" do
  enqueue(:two)
  Jobs.perform
  assert_equal [\"first?\"], Approvals.pending.map {{ |a| a[\"message\"] }}
  Approvals.approve(Approvals.pending.first[\"id\"])
  Jobs.perform
  assert_equal [\"second?\"], Approvals.pending.map {{ |a| a[\"message\"] }}
  assert_equal 1, Post.count
  Approvals.approve(Approvals.pending.first[\"id\"])
  Jobs.perform
  assert_equal [\"one\", \"two\"], Post.all.map {{ |p| p.title }}
end
test \"a test's human still decides outside jobs\" do
  with_human(approve_all) do
    approve! \"now?\"
  end
end
"
    ));
    for (name, error) in results {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
}
