//! Streaming in the language: a prompt call, an agent's `ask` and a
//! conversation's `say` give their answer to a block as it is written, and a
//! route answers as Server-Sent Events (`stream do |out| … end`), collected
//! by the `request` test helper.

mod common;

use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Provider, Response, run_main, run_tests};
use grenat_llm::{Delta, LlmError, Request, STOPPED, Sink};
use serde_json::json;

/// A provider that streams its replies in pieces of `size` characters
/// (text, then each tool call's input), a pause between two when `paced`,
/// and says whether it was stopped.
struct Chunked {
    replies: Mutex<VecDeque<Response>>,
    size: usize,
    paced: bool,
    stopped: AtomicBool,
}

impl Chunked {
    fn new(size: usize, replies: Vec<Response>) -> Arc<Chunked> {
        Arc::new(Chunked { replies: Mutex::new(replies.into()), size, paced: false, stopped: AtomicBool::new(false) })
    }

    fn next(&self) -> Result<Response, LlmError> {
        self.replies.lock().unwrap().pop_front().ok_or(LlmError::new("no reply left"))
    }

    fn pieces(&self, text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        chars.chunks(self.size).map(|c| c.iter().collect()).collect()
    }
}

impl Provider for Chunked {
    fn complete(&self, _: &Request) -> Result<Response, LlmError> {
        self.next()
    }

    fn stream(&self, _: &Request, sink: &mut Sink) -> Result<Response, LlmError> {
        let response = self.next()?;
        let mut deltas: Vec<(Option<(String, String)>, String)> =
            self.pieces(&response.text()).into_iter().map(|p| (None, p)).collect();
        for call in response.tool_uses() {
            for piece in self.pieces(&call.input.to_string()) {
                deltas.push((Some((call.id.clone(), call.name.clone())), piece));
            }
        }
        for (call, piece) in &deltas {
            let delta = match call {
                None => Delta::Text(piece),
                Some((id, name)) => Delta::ToolInput { id, name, json: piece },
            };
            if self.paced {
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            if sink(delta) == ControlFlow::Break(()) {
                self.stopped.store(true, Ordering::Relaxed);
                // billed as far as it went: here, as a whole
                return Err(LlmError { billed: Some(Box::new(response.clone())), ..LlmError::new(STOPPED) });
            }
        }
        Ok(response)
    }
}

/// Runs `main` of `src` against `provider`: (output, error).
fn run_chunked(src: &str, provider: Arc<Chunked>) -> (String, Option<String>) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let buffer = Arc::new(Mutex::new(String::new()));
    let options = Options {
        provider: Some(provider),
        output: Output::Capture(buffer.clone()),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    let result = run_main(&parsed.program, Vec::new(), options);
    let output = buffer.lock().unwrap().clone();
    (output, result.err().map(|e| format!("{}: {}", e.ty, e.message)))
}

const MODEL: &str = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\n";

const REPLY: &str = "\
prompt reply(q: String) -> ~String using :fast
  user q
end
";

#[test]
fn a_prompt_call_with_a_block_gets_its_answer_piece_by_piece() {
    let src = format!(
        "{MODEL}{REPLY}
def main uses llm
  chunks = []
  untrusted = []
  answer = reply(\"hi\") {{ |chunk|
    chunks << chunk
    untrusted << chunk.tainted?
  }}
  puts chunks.join(\"|\")
  puts answer
  puts untrusted.all? {{ |u| u }}
  puts answer.tainted?
end
"
    );
    let provider = Chunked::new(4, vec![Response::text_reply("Bonjour, ça va ?")]);
    let (output, error) = run_chunked(&src, provider);
    assert_eq!(error, None);
    assert_eq!(output, "Bonj|our,| ça |va ?\nBonjour, ça va ?\ntrue\ntrue\n");
}

#[test]
fn a_block_that_fails_stops_the_model_s_stream() {
    let src = format!(
        "{MODEL}{REPLY}
def main uses llm
  n = 0
  begin
    reply(\"hi\") {{ |chunk|
      n += 1
      raise ArgumentError, \"enough\" if n == 2
    }}
  rescue ArgumentError => e
    puts \"stopped after #{{n}}: #{{e.message}}\"
  end
end
"
    );
    // as a network gives them: the block fails before the answer is whole
    let provider = Arc::new(Chunked {
        replies: Mutex::new(vec![Response::text_reply("a long answer")].into()),
        size: 2,
        paced: true,
        stopped: AtomicBool::new(false),
    });
    let (output, error) = run_chunked(&src, provider.clone());
    assert_eq!(error, None);
    assert_eq!(output, "stopped after 2: enough\n");
    // the provider was told: no more of the answer is read
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !provider.stopped.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(provider.stopped.load(Ordering::Relaxed));
}

#[test]
fn a_call_its_block_stopped_is_counted_by_budgets() {
    let src = format!(
        "{MODEL}{REPLY}
def main uses llm
  within budget(usd: 0.15) do
    reply(\"x\") {{ |c| }}
    begin
      reply(\"x\") {{ |c| raise ArgumentError, \"stop\" }}
    rescue ArgumentError => e
      puts \"stopped: #{{e.message}}\"
    end
    puts budget.spent
    reply(\"x\") {{ |c| }}
    puts \"a third call\"
  end
end
"
    );
    // 100,000 tokens in at $1 a million: $0.10 each
    let answer = || Response::text_reply("a long answer").with_usage(100_000, 0);
    let provider = Chunked::new(2, vec![answer(), answer(), answer()]);
    let (output, error) = run_chunked(&src, provider);
    assert_eq!(output, "stopped: stop\n$0.20\n");
    assert_eq!(error.as_deref(), Some("BudgetExceeded: budget exceeded: $0.20 spent, limit $0.15"));
}

#[test]
fn only_a_text_answer_streams() {
    let src = format!(
        "{SUMMARY}
def main uses llm
  summarize(\"x\") {{ |chunk| puts chunk }}
end
"
    );
    let (_, error) = run_chunked(&src, Chunked::new(4, vec![summary_reply()]));
    assert_eq!(
        error.unwrap(),
        "TypeError: `summarize` answers a structure: only a prompt answering a `String` streams"
    );
}

const DESK: &str = "\
## Looks a ticket up.
tool lookup(id: Int) -> String
  \"ticket #{id}: refund asked\"
end
struct Verdict
  ok: Bool
end
agent Desk
  model :fast
  tools lookup
  on Chat(message: String) -> ~String
    run message
  end
  on Judge(message: String) -> ~Verdict
    run message
  end
end
";

#[test]
fn an_ask_with_a_block_gets_the_final_answer_as_it_is_written() {
    let answer = "Refund \"approved\"\nfor #42 ✓";
    let src = format!(
        "{MODEL}{DESK}
def main uses llm
  desk = spawn Desk
  chunks = []
  reply = desk.ask(Chat(message: \"refund #42?\")) {{ |chunk| chunks << chunk }}
  puts chunks.size > 1
  puts chunks.join == reply
  puts reply
end
"
    );
    let provider = Chunked::new(
        3,
        vec![
            // text and a tool call first: not the answer, not streamed
            Response::from_content(
                vec![
                    json!({"type": "text", "text": "Looking it up."}),
                    json!({"type": "tool_use", "id": "t1", "name": "lookup", "input": {"id": 42}}),
                ],
                "tool_use",
            ),
            Response::tool_call("t2", "final_answer", json!({"value": answer})),
        ],
    );
    let (output, error) = run_chunked(&src, provider);
    assert_eq!(error, None);
    assert_eq!(output, format!("true\ntrue\n{answer}\n"));
}

#[test]
fn a_structured_answer_or_a_tell_does_not_stream() {
    let src = format!(
        "{MODEL}{DESK}
def main uses llm
  desk = spawn Desk
  begin
    desk.tell(Chat(message: \"x\")) {{ |chunk| puts chunk }}
  rescue ArgumentError => e
    puts e.message
  end
  desk.ask(Judge(message: \"x\")) {{ |chunk| puts chunk }}
end
"
    );
    let provider = Chunked::new(3, vec![Response::tool_call("t", "final_answer", json!({"ok": true}))]);
    let (output, error) = run_chunked(&src, provider);
    assert_eq!(output, "`tell` waits for no answer: only `ask` streams one to a block\n");
    assert_eq!(
        error.unwrap(),
        "TypeError: `Desk` answers `Judge` with a structure: only an answer that is a `String` streams"
    );
}

#[test]
fn a_conversation_streams_what_it_says() {
    let src = format!(
        "{MODEL}
def main uses llm
  chat = Conversation.new(model: :fast)
  parts = []
  reply = chat.say(\"Hello\") {{ |chunk| parts << chunk }}
  puts parts.join(\"/\")
  puts chat.history.last[\"text\"] == reply
end
"
    );
    let (output, error) = run_chunked(&src, Chunked::new(5, vec![Response::text_reply("Hi there, Ada")]));
    assert_eq!(error, None);
    assert_eq!(output, "Hi th/ere, /Ada\ntrue\n");
}

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

#[test]
fn a_route_streams_events_and_a_test_reads_them() {
    let src = format!(
        "{MODEL}{REPLY}
get \"/chat\" do |req|
  stream do |out|
    reply(req.params[\"q\"]) {{ |chunk| out << chunk }}
    out.event(\"done\", {{\"chars\" => 12}})
  end
end

get \"/lines\" do |req|
  status 202, stream {{ |out| out << \"two\\nlines\" }}
end

get \"/broken\" do |req|
  stream do |out|
    out << \"start\"
    raise ArgumentError, \"mid-stream\"
  end
end

test \"the answer arrives as events\" do
  mock :fast, replies: [\"Hello, world\"]
  r = request(:get, \"/chat?q=hi\")
  assert_equal 200, r[\"status\"]
  assert_equal \"text/event-stream\", r[\"content_type\"]
  assert_equal [\"message\", \"done\"], r[\"events\"].map {{ |e| e[\"event\"] }}
  assert_equal \"Hello, world\", r[\"events\"].first[\"data\"]
  assert_equal \"{{\\\"chars\\\":12}}\", r[\"events\"].last[\"data\"]
  assert_equal \"data: Hello, world\\n\\nevent: done\\ndata: {{\\\"chars\\\":12}}\\n\\n\", r[\"body\"]
end

test \"lines of data cannot forge an event\" do
  r = request(:get, \"/lines\")
  assert_equal 202, r[\"status\"]
  assert_equal \"data: two\\ndata: lines\\n\\n\", r[\"body\"]
  assert_equal [\"two\\nlines\"], r[\"events\"].map {{ |e| e[\"data\"] }}
end

test \"a failure mid-stream is the test's\" do
  e = assert_raises(ArgumentError) {{ request(:get, \"/broken\") }}
  assert_equal \"mid-stream\", e.message
end
"
    );
    let results = results(&src);
    assert_eq!(results.len(), 3);
    for (name, error) in results {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
}

#[test]
fn an_event_s_name_is_the_program_s_own_and_no_secret_is_sent() {
    let src = "\
get \"/named\" do |req|
  stream { |out| out.event(req.params[\"name\"], \"x\") }
end
get \"/multiline\" do |req|
  stream { |out| out.event(\"a\\nb\", \"x\") }
end
get \"/secret\" do |req|
  stream { |out| out << Credentials.fetch(:api, :token) }
end
test \"named\" do
  request(:get, \"/named?name=admin\")
end
test \"multiline\" do
  request(:get, \"/multiline\")
end
test \"secret\" do
  mock_credentials({\"api\" => {\"token\" => \"s3cret\"}})
  request(:get, \"/secret\")
end
";
    let errors: Vec<String> = results(src).into_iter().map(|(_, e)| e.unwrap_or_default()).collect();
    assert_eq!(
        errors,
        [
            "TaintError: an untrusted value names an event: name it yourself",
            "ArgumentError: an event's name is one line, not empty: \"a\\nb\"",
            "SecretError: a secret is never sent to a client",
        ]
    );
}
