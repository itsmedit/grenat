//! Helpers shared by the runtime tests: scripted runs and fixtures.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use grenat_interp::{Options, Output, Response, RuntimeError, Scripted, Summary, run_main};
use serde_json::json;

pub struct Run {
    pub result: Result<Summary, RuntimeError>,
    pub output: String,
    pub requests: Vec<serde_json::Value>,
    pub elapsed: Duration,
}

impl Run {
    /// Output of a run that must succeed.
    pub fn ok(self) -> String {
        if let Err(e) = &self.result {
            panic!("error: {e:?}\noutput:\n{}", self.output);
        }
        self.output
    }

    pub fn err(self) -> RuntimeError {
        match self.result {
            Ok(_) => panic!("expected an error\noutput:\n{}", self.output),
            Err(e) => e,
        }
    }
}

/// Runs `src` with the given provider, on a 512 MB stack like the CLI.
/// How to run a program in tests.
#[derive(Clone, Copy)]
pub struct Mode {
    pub jit: bool,
    pub log: bool,
}

impl Default for Mode {
    fn default() -> Self {
        Mode { jit: true, log: false }
    }
}

/// Runs `src` with a given provider.
pub fn run_provider(src: &str, provider: Scripted, input: &[&str], args: &[&str]) -> Run {
    run_mode(src, provider, input, args, Mode::default())
}

pub fn run_mode(src: &str, provider: Scripted, input: &[&str], args: &[&str], mode: Mode) -> Run {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let provider = Arc::new(provider);
    let buffer = Arc::new(Mutex::new(String::new()));
    let options = Options {
        provider: Some(provider.clone()),
        output: Output::Capture(buffer.clone()),
        input: Some(input.iter().map(|s| s.to_string()).collect::<VecDeque<_>>()),
        log: mode.log,
        jit: mode.jit,
    };
    let started = Instant::now();
    // run_main runs the interpreter on its own large stack
    let result = run_main(&parsed.program, args.iter().map(|s| s.to_string()).collect(), options);
    let elapsed = started.elapsed();
    let output = buffer.lock().unwrap().clone();
    Run { result, output, requests: provider.requests(), elapsed }
}

pub fn run_full(src: &str, replies: Vec<Response>, input: &[&str], args: &[&str]) -> Run {
    run_provider(src, Scripted::new(replies), input, args)
}

pub fn run_with(src: &str, replies: Vec<Response>, input: &[&str]) -> Run {
    run_full(src, replies, input, &["arg1"])
}

pub fn run(src: &str) -> String {
    run_with(src, Vec::new(), &[]).ok()
}

pub fn run_err(src: &str, replies: Vec<Response>) -> RuntimeError {
    run_with(src, replies, &[]).err()
}

pub fn example(name: &str) -> String {
    std::fs::read_to_string(format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

// ── Fixtures ─────────────────────────────────────────────────

pub const SUMMARY: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\", temperature: 0.2
enum Sentiment
  Positive ## Favourable tone
  Negative
end
## A summary.
struct Summary
  title: String ## Short title
  bullets: Array(String)
  sentiment: Sentiment
end
## Summarizes an article.
prompt summarize(article: String) -> ~Summary using :fast
  system \"Be concise.\"
  user \"Article: #{article}\"
end
";

pub fn summary_reply() -> Response {
    Response::json_reply(json!({"title": "Cat", "bullets": ["sleeps"], "sentiment": "Positive"}))
}

pub const MAILER: &str = "\
tool send(to: String, body: String) -> Unit uses net(\"smtp.example.com\")
  puts \"sent to #{to}: #{body}\"
end
";

pub const READER: &str = "\
model :smart, provider: :anthropic, name: \"claude-opus-5\"
struct Report
  answer: String
  files: Array(String)
end
## Reads a file.
tool read_file(path: String, max_lines: Int = 10) -> String uses fs.read
  \"contents of #{path} (#{max_lines} lines)\"
end
agent Reader
  model :smart
  tools read_file
  max_turns 4
  instructions \"You read files.\"
  @asked: Int = 0
  on Ask(question: String) -> ~Report
    @asked += 1
    run \"Question: #{question}\"
  end
  on Asked -> Int
    @asked
  end
end
";
