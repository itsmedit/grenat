//! Durable workflows: steps journaled, a failed run resumed without calling
//! the model again, a completed run replayed.

mod common;

use common::*;
use grenat_interp::{Response, Scripted};

const PUBLISH: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"

prompt research(topic: String) -> ~String using :fast
  user \"research #{topic}\"
end

prompt write(notes: String) -> ~String using :fast
  user \"write from #{notes}\"
end

workflow publish(topic: String) -> String
  notes = step(:research) { research(topic).trust! }
  draft = step(:write) { write(notes).trust! }
  step(:publish) { \"published: #{draft}\" }
end

def main
  puts publish(\"otters\")
end
";

fn run_in(journal: &std::path::Path, replies: Vec<Response>) -> Run {
    let mode = Mode { log: true, journal: Some(journal.to_path_buf()), ..Mode::default() };
    run_mode(PUBLISH, Scripted::new(replies), &[], &[], mode)
}

#[test]
fn an_interrupted_workflow_resumes_without_calling_the_model_again() {
    let journal = temp_dir("publish");
    // first run: the second call fails (no reply left)
    let first = run_in(&journal, vec![Response::text_reply("notes")]);
    assert_eq!(first.requests.len(), 2);
    assert_eq!(first.err().ty, "LlmError");

    // second run: `research` comes from the journal, only `write` calls the model
    let second = run_in(&journal, vec![Response::text_reply("a draft")]);
    assert!(second.output.contains("[workflow] publish: resumed, 1 step(s) journaled"), "{}", second.output);
    assert!(second.output.contains("step :research replayed"), "{}", second.output);
    assert!(second.output.ends_with("published: a draft\n"), "{}", second.output);
    assert_eq!(second.requests.len(), 1);
    assert!(second.requests[0]["messages"][0]["content"].as_str().unwrap().contains("write from notes"));

    // third run: completed, the result is replayed
    let third = run_in(&journal, vec![]);
    assert!(third.output.contains("completed earlier, result replayed"), "{}", third.output);
    assert!(third.output.ends_with("published: a draft\n"), "{}", third.output);
    assert!(third.requests.is_empty());
}

#[test]
fn journaled_values_come_back_with_their_types() {
    let src = "\
struct Point
  x: Float
  y: Float
end

enum Shape
  Circle(radius: Float)
  Square(side: Int)
end

workflow collect(n: Int) -> Array(String)
  a = step(:float) { 2.0 * n }
  b = step(:struct) { Point(x: 1.5, y: -2.0) }
  c = step(:enum) { Circle(radius: 0.5) }
  d = step(:hash) { {\"k\" => [1, :sym, nil]} }
  [a.inspect, b.inspect, c.inspect, d.inspect]
end

def main
  p collect(3)
end
";
    let journal = temp_dir("types");
    let mode = Mode { journal: Some(journal.clone()), ..Mode::default() };
    let first = run_mode(src, Scripted::new([]), &[], &[], mode.clone());
    // the second run replays every step from the journal
    let second = run_mode(src, Scripted::new([]), &[], &[], mode);
    let expected = "[\"6.0\", \"Point(x: 1.5, y: -2.0)\", \"Circle(radius: 0.5)\", \"{\\\"k\\\" => [1, :sym, nil]}\"]\n";
    assert_eq!(first.ok(), expected);
    assert_eq!(second.ok(), expected);
}

#[test]
fn runs_with_other_arguments_are_other_runs() {
    let src = "\
workflow twice(n: Int) -> Int
  step(:double) { n * 2 }
end

def main
  p [twice(1), twice(2), twice(1)]
end
";
    let journal = temp_dir("args");
    let mode = Mode { journal: Some(journal.clone()), ..Mode::default() };
    assert_eq!(run_mode(src, Scripted::new([]), &[], &[], mode).ok(), "[2, 4, 2]\n");
    assert_eq!(std::fs::read_dir(&journal).unwrap().count(), 2);
}

#[test]
fn a_step_must_return_data() {
    let src = "\
class Counter
  @n: Int = 0
end

workflow keep -> Int
  step(:object) { Counter.new }
  1
end

keep
";
    let e = run_mode(src, Scripted::new([]), &[], &[], Mode::default()).err();
    assert_eq!(e.ty, "TypeError");
    assert!(e.message.contains("step :object must return data"), "{}", e.message);
}
