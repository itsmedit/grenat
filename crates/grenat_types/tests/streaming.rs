//! Streaming, checked: the pieces of an answer are untrusted, only a
//! `String` answer streams, and a streamed response's events carry no
//! secret and are named by the program (E0200, E0412, E0414).

mod common;

use common::*;

const APP: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt reply(q: String) -> ~String using :fast
  user q
end
struct Verdict
  ok: Bool
end
prompt judge(q: String) -> ~Verdict using :fast
  user q
end
agent Desk
  model :fast
  on Chat(message: String) -> ~String
    run message
  end
  on Judge(message: String) -> ~Verdict
    run message
  end
end
tool send(to: String, body: String) -> Unit uses net
  puts body
end
";

#[test]
fn prompts_agents_conversations_and_routes_stream() {
    clean(&format!(
        "{APP}
def ask_all(q: String) -> String uses llm
  parts = []
  reply(q) {{ |chunk| parts << chunk.upcase }}
  desk = spawn Desk
  desk.ask(Chat(message: q)) {{ |chunk| print chunk }}
  Conversation.new(model: :fast).say(q) {{ |chunk| print chunk.size }}
  parts.join
end
get \"/chat\" do |req|
  stream do |out|
    reply(req.params[\"q\"]) {{ |chunk| out << chunk }}
    out << {{\"done\" => true}}
    out.event(\"done\", req.params[\"q\"])
  end
end
get \"/status\" do |req|
  status 202, stream {{ |out| out << \"ok\" }}
end
test \"a stream read\" do
  mock :fast, replies: [\"hi\"]
  r = request(:get, \"/chat?q=x\")
  assert_equal \"hi\", r[\"events\"].first[\"data\"]
end
"
    ));
}

#[test]
fn a_piece_of_an_answer_is_untrusted() {
    single(
        &format!("{APP}def f uses llm, net\n  reply(\"x\") {{ |chunk| send(\"a@b.c\", chunk) }}\nend\n"),
        "E0412",
        "chunk",
    );
    let desk = format!(
        "{APP}def f uses llm, net\n  desk = spawn Desk\n  desk.ask(Chat(message: \"x\")) {{ |c| send(\"a@b.c\", c) }}\nend\n"
    );
    single(&desk, "E0412", "c");
}

#[test]
fn only_a_text_answer_streams() {
    let d = single(
        &format!("{APP}def f uses llm\n  judge(\"x\") {{ |c| puts c }}\nend\n"),
        "E0200",
        "judge(\"x\") { |c| puts c }",
    );
    assert_eq!(d.message, "`judge` answers `Verdict`: only an answer that is a `String` streams");
    let d = single(
        &format!("{APP}def f uses llm\n  d = spawn Desk\n  d.ask(Judge(message: \"x\")) {{ |c| puts c }}\nend\n"),
        "E0200",
        "d.ask(Judge(message: \"x\")) { |c| puts c }",
    );
    assert_eq!(d.message, "`Desk#Judge` answers `Verdict`: only an answer that is a `String` streams");
    let d = single(
        &format!("{APP}def f uses llm\n  d = spawn Desk\n  d.tell(Chat(message: \"x\")) {{ |c| puts c }}\nend\n"),
        "E0200",
        "d.tell(Chat(message: \"x\")) { |c| puts c }",
    );
    assert_eq!(d.message, "`tell` waits for no answer: only `ask` streams one to a block");
    // any other function still takes no block
    single("def g = 1\ndef f\n  g { |x| x }\nend\n", "E0200", "g { |x| x }");
}

#[test]
fn an_event_is_named_by_the_program_and_carries_no_secret() {
    single(
        "get \"/x\" do |req|\n  stream { |out| out.event(req.params[\"n\"], \"x\") }\nend\n",
        "E0412",
        "req.params[\"n\"]",
    );
    single(
        "get \"/x\" do |req|\n  stream { |out| out << Credentials.fetch(:api, :token) }\nend\n",
        "E0414",
        "out << Credentials.fetch(:api, :token)",
    );
    single("get \"/x\" do |req|\n  stream { |out| out.send(\"x\") }\nend\n", "E0200", "out.send(\"x\")");
}
