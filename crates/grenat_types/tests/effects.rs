//! Declared and inferred effects (E0300, E0500).

mod common;

use common::*;

#[test]
fn main_must_declare_what_it_does() {
    let src = format!("{PRELUDE}def main\n  summarize(\"x\")\nend\n");
    let d = single(&src, "E0300", "summarize(\"x\")");
    assert_eq!(d.help.as_deref(), Some("add `uses llm`"));
    clean(&format!("{PRELUDE}def main uses llm\n  summarize(\"x\")\nend\n"));
}

#[test]
fn declared_effects_must_cover_the_body() {
    let src = "def load uses fs.read(\"./docs\")\n  File.read(\"./secrets/key\")\nend\n";
    let d = single(src, "E0300", "File.read(\"./secrets/key\")");
    assert!(d.message.contains("fs.read(\"./secrets/key\")"), "{}", d.message);
    clean("def load uses fs.read(\"./docs\")\n  File.read(\"./docs/a.md\")\nend\n");
    clean("def load uses fs\n  File.write(\"a\", \"b\")\nend\n");
}

#[test]
fn effects_propagate_through_undeclared_helpers() {
    let src = "def helper = File.read(\"a\")\ndef main uses llm\n  helper\nend\n";
    let d = single(src, "E0300", "helper");
    assert!(d.message.contains("fs.read(\"a\")"), "{}", d.message);
}

#[test]
fn tools_must_declare_effects() {
    let src = "tool read(path: String) -> String\n  File.read(path)\nend\n";
    single(src, "E0300", "File.read(path)");
}

#[test]
fn unknown_effect_names_are_reported() {
    let d = single("def f uses fs.raed\n  1\nend\n", "E0500", "fs.raed");
    assert_eq!(d.help.as_deref(), Some("did you mean `fs.read`?"));
}

#[test]
fn in_a_workflow_non_deterministic_effects_are_steps() {
    // the model call would happen again, differently, on resume
    let src = format!(
        "{PRELUDE}workflow publish(topic: String) -> String uses llm\n  s = summarize(topic).trust!.title\n  step(:tell) {{ s }}\nend\n"
    );
    let d = single(&src, "E0310", "summarize(topic)");
    assert!(d.message.contains("`llm` outside a `step` in workflow `publish`"), "{}", d.message);
    // inside a step, its result is journaled
    clean(&format!(
        "{PRELUDE}workflow publish(topic: String) -> String uses llm\n  s = step(:summary) {{ summarize(topic).trust!.title }}\n  step(:tell) {{ s }}\nend\n"
    ));
    // deterministic code needs no step; functions outside workflows are free
    clean(&format!("{PRELUDE}workflow twice(n: Int) -> Int\n  n * 2\nend\ndef free(t: String) -> String uses llm = summarize(t).trust!.title\n"));
}

#[test]
fn test_doubles_and_evals_are_known() {
    clean(&format!(
        "{PRELUDE}test \"doubles\" do\n  mock :fast, replies: [\"x\"]\n  data = fixture(\"a.json\")\n  cassette \"c\" do\n    summarize(\"x\")\n  end\nend\neval \"quality\", dataset: \"rows.jsonl\", threshold: 0.5 do |row|\n  judge(:fast, \"Faithful?\", row.text) > 0.5\nend\n"
    ));
    let d = single(&format!("{PRELUDE}def grade(t: String) -> Float = judge(\"Good?\", t)\ndef main\n  grade(\"x\")\nend\n"), "E0300", "grade(\"x\")");
    assert!(d.message.contains("llm"), "{}", d.message);
}

#[test]
fn http_requests_are_net_effects_on_their_host() {
    let d = single("def main\n  Http.get(\"https://api.github.com/repos/x\")\nend\n", "E0300", "Http.get(\"https://api.github.com/repos/x\")");
    assert!(d.message.contains("net(\"api.github.com\")"), "{}", d.message);
    clean("def main uses net(\"api.github.com\")\n  p Http.get(\"https://api.github.com/repos/x\").status\nend\n");
    let d = single(
        "def main uses net(\"api.github.com\")\n  Http.get(\"https://evil.io/x\")\nend\n",
        "E0300",
        "Http.get(\"https://evil.io/x\")",
    );
    assert!(d.message.contains("net(\"evil.io\")"), "{}", d.message);
    // a URL built at run time is checked at run time
    clean("def main(args: Array(String)) uses net(\"api.github.com\")\n  Http.get(\"https://#{args.first}/x\")\nend\n");
    clean("test \"t\" do\n  mock_http \"GET https://x.io/*\", status: 200, json: {a: 1}\nend\n");
}

#[test]
fn databases_are_read_and_write_effects() {
    let d = single(
        "def main\n  db = Db.connect(\"sqlite::memory:\")\n  db.execute(\"DELETE FROM t\")\nend\n",
        "E0300",
        "db.execute(\"DELETE FROM t\")",
    );
    assert!(d.message.contains("db.write"), "{}", d.message);
    clean("def main uses db\n  db = Db.connect(\"sqlite::memory:\")\n  db.execute(\"DELETE FROM t\")\n  p db.query(\"SELECT 1\")\nend\n");
    single(
        "def count(db: Database) -> Int uses db.read\n  db.execute(\"DELETE FROM t\")\nend\n",
        "E0300",
        "db.execute(\"DELETE FROM t\")",
    );
}

#[test]
fn rows_are_typed_by_the_record_they_are_read_as() {
    let head = "struct Order\n  id: Int\n  total: Float\nend\ndef main uses db\n  db = Db.connect(\"sqlite::memory:\")\n";
    clean(&format!("{head}  p db.query(\"SELECT * FROM orders\", as: Order).map {{ |o| o.total }}.sum\nend\n"));
    let d = single(&format!("{head}  p db.query(\"SELECT * FROM orders\", as: Order).first.nope\nend\n"), "E0200", "nope");
    assert!(d.message.contains("`nope`"), "{}", d.message);
}

#[test]
fn programs_are_shell_effects_restricted_by_program() {
    let d = single("def main\n  Shell.run([\"git\", \"status\"])\nend\n", "E0300", "Shell.run([\"git\", \"status\"])");
    assert!(d.message.contains("shell(\"git\")"), "{}", d.message);
    clean("def main uses shell(\"git\")\n  p Shell.run([\"git\", \"status\"]).ok?\nend\n");
    single("def main uses shell(\"git\")\n  Shell.run([\"rm\", \"-rf\", \"x\"])\nend\n", "E0300", "Shell.run([\"rm\", \"-rf\", \"x\"])");
    clean("test \"t\" do\n  mock_shell \"git *\", stdout: \"ok\"\nend\n");
}

#[test]
fn tool_timeouts_are_durations() {
    let head = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\nagent A\n  model :fast\n";
    clean(&format!("{head}  tool_timeout 30\nend\n"));
    single(&format!("{head}  tool_timeout \"soon\"\nend\n"), "E0200", "\"soon\"");
}

#[test]
fn mcp_servers_are_capabilities() {
    let head = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\nmcp :linear, url: \"https://mcp.linear.app/mcp\"\nagent Pm\n  model :fast\n  tools mcp(:linear, only: [\"create_issue\"])\n  on Plan -> ~String\n    run \"plan\"\n  end\nend\n";
    let d = single(&format!("{head}def main uses llm\n  spawn(Pm).ask(Plan())\nend\n"), "E0300", "spawn(Pm).ask(Plan())");
    assert!(d.message.contains("mcp(\"linear\")"), "{}", d.message);
    clean(&format!("{head}def main uses llm, mcp(\"linear\")\n  spawn(Pm).ask(Plan())\nend\n"));
    single("def main uses mcp(\"notion\")\n  Mcp.tools(:linear)\nend\n", "E0300", "Mcp.tools(:linear)");
    clean("test \"t\" do\n  mock_mcp :linear, tools: {\"a\" => \"b\"}\nend\n");
}

#[test]
fn reading_an_attachment_is_reading_a_file() {
    let prompt = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\nprompt read(doc: Attachment) -> ~String using :fast\n  user \"Summarize.\", doc\nend\n";
    let d = single(&format!("{prompt}def main uses llm\n  read(Pdf.read(\"./a.pdf\"))\nend\n"), "E0300", "Pdf.read(\"./a.pdf\")");
    assert!(d.message.contains("fs.read"), "{}", d.message);
    clean(&format!("{prompt}def main uses llm, fs.read\n  read(Pdf.read(\"./a.pdf\"))\n  read(Image.url(\"https://x.io/a.png\"))\nend\n"));
}

#[test]
fn a_conversation_is_a_model_and_its_answers_are_untrusted() {
    let head = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\n";
    let d = single(&format!("{head}def main\n  Conversation.new(model: :fast).say(\"hi\")\nend\n"), "E0300", "Conversation.new(model: :fast).say(\"hi\")");
    assert!(d.message.contains("llm"), "{}", d.message);
    single(
        &format!("{head}def main uses llm, net\n  a = Conversation.new(model: :fast).say(\"hi\")\n  Http.post(\"https://x.io\", body: a)\nend\n"),
        "E0412",
        "a",
    );
}

#[test]
fn records_are_typed_and_are_database_effects() {
    let head = "struct Ticket\n  table :tickets\n  id: Int?\n  subject: String\nend\n";
    clean(&format!("{head}def titles -> Array(String) uses db.read = Ticket.all.map {{ |t| t.subject }}\n"));
    single(&format!("{head}def main\n  Ticket.create(subject: \"x\")\nend\n"), "E0300", "Ticket.create(subject: \"x\")");
    single(&format!("{head}def f uses db.read\n  Ticket.find(1).subject.nope\nend\n"), "E0200", "nope");
    single(&format!("{head}def f(t: Ticket) uses db.read\n  t.delete\nend\n"), "E0300", "t.delete");
    single(
        &format!("{head}post \"/t\" do |req|\n  Ticket.create(subject: req.json[\"s\"])\nend\n"),
        "E0412",
        "req.json[\"s\"]",
    );
}
