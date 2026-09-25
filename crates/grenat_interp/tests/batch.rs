//! `batch_map`: model calls grouped into batches, round by round.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Provider, Response, run_main, run_tests};
use grenat_llm::{LlmError, Request};

/// Answers with the request's text, upper-cased; records each batch's size.
#[derive(Default)]
struct Recorder {
    batches: Mutex<Vec<usize>>,
    singles: Mutex<usize>,
}

impl Provider for Recorder {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        *self.singles.lock().unwrap() += 1;
        let text = request.messages[0]["content"].as_str().unwrap_or_default().to_uppercase();
        Ok(Response::text_reply(text).with_usage(1_000_000, 0))
    }

    fn batch(&self, requests: &[Request]) -> Result<Vec<Result<Response, LlmError>>, LlmError> {
        self.batches.lock().unwrap().push(requests.len());
        let answers = requests.iter().map(|r| self.complete(r)).collect();
        *self.singles.lock().unwrap() -= requests.len();
        Ok(answers)
    }
}

const PROMPT: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt shout(text: String) -> ~String using :fast
  user text
end
";

fn run_recorded(src: &str) -> (String, Result<grenat_interp::Summary, grenat_interp::RuntimeError>, Arc<Recorder>) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let recorder = Arc::new(Recorder::default());
    let buffer = Arc::new(Mutex::new(String::new()));
    let options = Options {
        provider: Some(recorder.clone()),
        output: Output::Capture(buffer.clone()),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    let result = run_main(&parsed.program, Vec::new(), options);
    let output = buffer.lock().unwrap().clone();
    (output, result, recorder)
}

#[test]
fn one_batch_per_round_answers_in_order_at_half_the_price() {
    let (out, result, recorder) = run_recorded(&format!(
        "{PROMPT}words = [\"a\", \"b\", \"c\", \"d\", \"e\"]\np words.batch_map {{ |w| shout(w).trust! }}\np budget.spent\n"
    ));
    let summary = result.unwrap();
    assert_eq!(out, "[\"A\", \"B\", \"C\", \"D\", \"E\"]\n$2.50\n");
    assert_eq!(*recorder.batches.lock().unwrap(), [5]);
    assert_eq!(*recorder.singles.lock().unwrap(), 0);
    assert_eq!(summary.llm_calls, 5);
    // 5 × 1M input tokens of Haiku ($1/M), halved
    assert!((summary.cost_usd - 2.5).abs() < 1e-9, "{}", summary.cost_usd);
}

#[test]
fn a_second_call_is_a_second_round() {
    let (out, result, recorder) = run_recorded(&format!(
        "{PROMPT}p [\"a\", \"b\", \"c\"].batch_map {{ |w| shout(shout(w).trust! + \"x\").trust! }}\n"
    ));
    result.unwrap();
    assert_eq!(out, "[\"AX\", \"BX\", \"CX\"]\n");
    assert_eq!(*recorder.batches.lock().unwrap(), [3, 3]);
}

#[test]
fn elements_without_a_model_call_and_errors() {
    let (out, result, recorder) = run_recorded(&format!(
        "{PROMPT}p [1, 2, 3].batch_map {{ |n| if n.even? then n.to_s else shout(n.to_s).trust! end }}\n"
    ));
    result.unwrap();
    assert_eq!(out, "[\"1\", \"2\", \"3\"]\n");
    assert_eq!(*recorder.batches.lock().unwrap(), [2]);
    let (_, result, recorder) = run_recorded(&format!(
        "{PROMPT}p [1, 2, 3].batch_map {{ |n| raise \"bad #{{n}}\" if n == 2\n shout(n.to_s).trust! }}\n"
    ));
    let e = result.unwrap_err();
    assert_eq!(e.message, "bad 2");
    // the others were still answered, in one batch
    assert_eq!(*recorder.batches.lock().unwrap(), [2]);
}

#[test]
fn tests_batch_through_mocks() {
    let src = format!(
        "{PROMPT}test \"mocked\" do\n  mock :fast, replies: [\"X\", \"Y\"]\n  assert_equal 2, [\"a\", \"b\"].batch_map {{ |w| shout(w).trust! }}.size\nend\n"
    );
    let parsed = grenat_parser::parse(&src);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    let outcomes = run_tests(&parsed.program, options).unwrap();
    assert!(outcomes[0].error.is_none(), "{:?}", outcomes[0].error);
}
