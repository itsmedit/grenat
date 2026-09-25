//! Test doubles: `mock`, `cassette`, `fixture`, `call`, and `grenat test`
//! never reaching a real model.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Response, Scripted, TestOutcome, run_tests};

/// Runs the tests of `src` from `dir`; `provider` stands for the real model.
fn tests_in(src: &str, dir: &Path, provider: Option<Arc<Scripted>>, record: bool) -> (Vec<TestOutcome>, String) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let buffer = Arc::new(Mutex::new(String::new()));
    let options = Options {
        provider: provider.map(|p| p as Arc<dyn grenat_interp::Provider>),
        output: Output::Capture(buffer.clone()),
        journal: Some(temp_dir("journal")),
        dir: Some(dir.to_path_buf()),
        record,
        ..Options::default()
    };
    let outcomes = run_tests(&parsed.program, options).unwrap();
    let output = buffer.lock().unwrap().clone();
    (outcomes, output)
}

/// Each test's name and error message (`None` when it passed).
fn results(outcomes: &[TestOutcome]) -> Vec<(String, Option<String>)> {
    outcomes.iter().map(|o| (o.name.clone(), o.error.as_ref().map(|e| format!("{}: {}", e.ty, e.message)))).collect()
}

fn passed(outcomes: &[TestOutcome]) {
    for (name, error) in results(outcomes) {
        assert!(error.is_none(), "`{name}` failed: {}", error.unwrap());
    }
}

#[test]
fn a_mock_answers_in_the_shape_each_call_expects() {
    let src = format!(
        "{SUMMARY}\
prompt title(text: String) -> ~String using :fast
  user text
end
prompt count(text: String) -> ~Int using :fast
  user text
end
test \"mocked\" do
  mock :fast, replies: [
    {{title: \"Cat\", bullets: [\"sleeps\"], sentiment: \"Positive\"}},
    \"A title\",
    3,
  ]
  s = summarize(\"...\")
  assert_equal \"Cat\", s.title
  assert s.sentiment == Positive
  assert_equal \"A title\", title(\"...\")
  assert_equal 3, count(\"...\")
end
"
    );
    let (outcomes, _) = tests_in(&src, &temp_dir("mock"), None, false);
    passed(&outcomes);
}

#[test]
fn a_mock_drives_an_agent_through_its_tools() {
    let src = format!(
        "{READER}\
test \"agent\" do
  mock :smart, replies: [
    call(:read_file, path: \"a.txt\", max_lines: 2),
    {{answer: \"done\", files: [\"a.txt\"]}},
  ]
  report = spawn(Reader).ask(Ask(question: \"?\"))
  assert_equal \"done\", report.answer
end
"
    );
    let (outcomes, _) = tests_in(&src, &temp_dir("agent"), None, false);
    passed(&outcomes);
}

#[test]
fn mock_errors_and_exhaustion_fail_the_call() {
    let src = format!(
        "{SUMMARY}\
test \"error\" do
  mock replies: [LlmError(\"overloaded\")]
  e = assert_raises(LlmError) {{ summarize(\"...\") }}
  assert_equal \"overloaded\", e.message
end
test \"exhausted\" do
  mock :fast, replies: []
  summarize(\"...\")
end
test \"mocks do not outlive their test\" do
  summarize(\"...\")
end
"
    );
    let (outcomes, _) = tests_in(&src, &temp_dir("errors"), None, false);
    let results = results(&outcomes);
    assert_eq!(results[0].1, None);
    assert_eq!(results[1].1.as_deref(), Some("LlmError: the mock of `:fast` has no reply left"));
    let offline = results[2].1.as_deref().unwrap();
    assert!(offline.starts_with("LlmError: no real model in tests: `claude-haiku-4-5`"), "{offline}");
}

#[test]
fn a_cassette_records_once_then_replays() {
    let dir = temp_dir("cassette");
    let src = format!(
        "{SUMMARY}\
test \"recorded\" do
  cassette \"summaries\" do
    assert_equal \"Cat\", summarize(\"about cats\").title
  end
end
"
    );
    let real = Arc::new(Scripted::new([summary_reply()]));
    let (outcomes, _) = tests_in(&src, &dir, Some(real.clone()), false);
    passed(&outcomes);
    assert_eq!(real.requests().len(), 1);
    assert!(dir.join("cassettes/summaries.json").exists());

    // replayed: no real model at all
    let (outcomes, _) = tests_in(&src, &dir, None, false);
    passed(&outcomes);

    // another request is not in the cassette
    let changed = src.replace("about cats", "about dogs");
    let (outcomes, _) = tests_in(&changed, &dir, None, false);
    let error = results(&outcomes)[0].1.clone().unwrap();
    assert!(error.contains("has no such call (re-record it with GRENAT_RECORD=1)"), "{error}");

    // recorded again on demand
    let real = Arc::new(Scripted::new([summary_reply()]));
    let (outcomes, _) = tests_in(&changed, &dir, Some(real.clone()), true);
    passed(&outcomes);
    assert_eq!(real.requests().len(), 1);
    let (outcomes, _) = tests_in(&changed, &dir, None, false);
    passed(&outcomes);
}

#[test]
fn a_failed_block_does_not_keep_its_recording() {
    let dir = temp_dir("failed");
    let src = format!(
        "{SUMMARY}\
test \"fails\" do
  cassette \"broken\" do
    summarize(\"x\")
    raise \"boom\"
  end
end
"
    );
    let real = Arc::new(Scripted::new([summary_reply()]));
    let (outcomes, _) = tests_in(&src, &dir, Some(real), false);
    assert_eq!(results(&outcomes)[0].1.as_deref(), Some("RuntimeError: boom"));
    assert!(!dir.join("cassettes/broken.json").exists());
}

#[test]
fn a_mock_wins_over_the_enclosing_cassette() {
    let dir = temp_dir("precedence");
    let src = format!(
        "{SUMMARY}\
test \"mock first\" do
  cassette \"unused\" do
    mock :fast, replies: [{{title: \"Mocked\", bullets: [], sentiment: \"Negative\"}}]
    assert_equal \"Mocked\", summarize(\"x\").title
  end
end
"
    );
    let real = Arc::new(Scripted::new(Vec::<Response>::new()));
    let (outcomes, _) = tests_in(&src, &dir, Some(real.clone()), false);
    passed(&outcomes);
    assert!(real.requests().is_empty());
}

#[test]
fn fixtures_are_read_from_the_fixtures_directory() {
    let dir = temp_dir("fixtures");
    std::fs::create_dir_all(dir.join("fixtures")).unwrap();
    std::fs::write(dir.join("fixtures/config.json"), r#"{"name": "demo", "limits": [1, 2.5]}"#).unwrap();
    std::fs::write(dir.join("fixtures/rows.jsonl"), "{\"q\": \"a\"}\n\n{\"q\": \"b\"}\n").unwrap();
    std::fs::write(dir.join("fixtures/mail.txt"), "Hello\n").unwrap();
    let src = "\
test \"fixtures\" do
  config = fixture(\"config.json\")
  assert_equal \"demo\", config[\"name\"]
  assert_equal [1, 2.5], config[\"limits\"]
  assert_equal [\"a\", \"b\"], fixture(\"rows.jsonl\").map { |r| r[\"q\"] }
  assert_equal \"Hello\\n\", fixture(\"mail.txt\")
  assert_raises(IOError) { fixture(\"missing.json\") }
end
";
    let (outcomes, _) = tests_in(src, &dir, None, false);
    passed(&outcomes);
}

#[test]
fn call_scripts_a_tool_call_checked_like_the_model_s() {
    let src = format!(
        "{READER}\
def helper(x: Int) -> Int = x
test \"bad arguments are reported to the model\" do
  mock :smart, replies: [
    call(:read_file, path: 42),
    {{answer: \"gave up\", files: []}},
  ]
  assert_equal \"gave up\", spawn(Reader).ask(Ask(question: \"?\")).answer
end
test \"only tools\" do
  assert_raises(TypeError) {{ call(:helper, x: 1) }}
  assert_raises(NameError) {{ call(:nope) }}
  assert_raises(ArgumentError) {{ call(\"read_file\") }}
end
"
    );
    let dir = temp_dir("call");
    let (outcomes, output) = tests_in(&src, &dir, None, false);
    passed(&outcomes);
    assert_eq!(output, "");
}
