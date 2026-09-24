//! Exécution de programmes complets, avec un fournisseur LLM scripté.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use grenat_interp::{Options, Output, Response, RuntimeError, Scripted, run_main, run_tests};
use serde_json::json;

struct Run {
    result: Result<grenat_interp::Summary, RuntimeError>,
    output: String,
    requests: Vec<serde_json::Value>,
}

fn run_with(src: &str, replies: Vec<Response>, input: &[&str]) -> Run {
    run_full(src, replies, input, &["arg1"])
}

/// Exécute sur une pile de 512 Mo, comme la CLI.
fn run_full(src: &str, replies: Vec<Response>, input: &[&str], args: &[&str]) -> Run {
    let src = src.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let input: VecDeque<String> = input.iter().map(|s| s.to_string()).collect();
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(move || {
            let parsed = grenat_parser::parse(&src);
            assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
            let provider = Rc::new(Scripted::new(replies));
            let buffer = Rc::new(RefCell::new(String::new()));
            let options = Options {
                provider: Some(provider.clone()),
                output: Output::Capture(buffer.clone()),
                input: Some(input),
                log: false,
            };
            let result = run_main(&parsed.program, args, options);
            let output = buffer.borrow().clone();
            Run { result, output, requests: provider.requests() }
        })
        .unwrap()
        .join()
        .unwrap()
}

fn run(src: &str) -> String {
    let r = run_with(src, Vec::new(), &[]);
    if let Err(e) = &r.result {
        panic!("erreur : {e:?}\nsortie :\n{}", r.output);
    }
    r.output
}

fn run_err(src: &str, replies: Vec<Response>) -> RuntimeError {
    run_with(src, replies, &[]).result.expect_err("une erreur était attendue")
}

// ── Langage ──────────────────────────────────────────────────

#[test]
fn closures_capture_and_mutate_outer_variables() {
    assert_eq!(run("total = 0\n[1, 2, 3].each { |x| total += x }\nputs total\n"), "6\n");
}

#[test]
fn return_inside_a_block_returns_from_the_method() {
    let src = "def first_even(xs: Array(Int)) -> Int?\n  xs.each { |x| return x if x.even? }\n  nil\nend\np first_even([1, 3, 4, 5])\n";
    assert_eq!(run(src), "4\n");
}

#[test]
fn main_receives_arguments() {
    assert_eq!(run("def main(args: Array(String))\n  puts args.first\nend\n"), "arg1\n");
}

#[test]
fn modules_are_included() {
    let src = "\
module Describable
  abstract def describe -> String
  def shout = describe.upcase
end
struct Invoice
  include Describable
  amount: Int
  def describe = \"facture de #{amount}\"
end
puts Invoice(amount: 3).shout
";
    assert_eq!(run(src), "FACTURE DE 3\n");
}

#[test]
fn structs_are_immutable_but_copyable() {
    let src = "struct P\n  x: Int\n  y: Int = 0\nend\na = P(x: 1)\nb = a.with(y: 2)\np a, b\n";
    assert_eq!(run(src), "P(x: 1, y: 0)\nP(x: 1, y: 2)\n");
    let e = run_err("struct P\n  x: Int\nend\na = P(x: 1)\na.x = 2\n", vec![]);
    assert_eq!(e.ty, "TypeError");
}

#[test]
fn case_without_match_raises() {
    let e = run_err("enum E\n  A\n  B\nend\ncase B\nin A then 1\nend\n", vec![]);
    assert_eq!(e.ty, "NoMatchingPattern");
}

#[test]
fn errors_carry_location_and_trace() {
    let src = "def inner\n  inconnue\nend\ndef outer = inner\nouter\n";
    let e = run_err(src, vec![]);
    assert_eq!(e.ty, "NameError");
    assert_eq!(&src[e.span.unwrap().range()], "inconnue");
    let names: Vec<_> = e.trace.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["inner", "outer"]);
}

#[test]
fn result_and_try_operator() {
    let src = "\
def parse(s: String) -> Result(Int, ParseError)
  return Err(ParseError(\"vide\")) if s.empty?
  Ok(s.to_i)
end
def double(s: String) = parse(s)? * 2
p double(\"21\")
begin
  double(\"\")
rescue ParseError => e
  puts e.message
end
p parse(\"\").or_else { |e| -1 }
";
    assert_eq!(run(src), "42\nvide\n-1\n");
}

#[test]
fn deep_recursion_is_reported_not_crashed() {
    let e = run_err("def f(n: Int) = f(n + 1)\nf(0)\n", vec![]);
    assert_eq!(e.ty, "StackOverflow");
}

// ── Prompts ──────────────────────────────────────────────────

const SUMMARY: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\", temperature: 0.2
enum Sentiment
  Positive ## Ton favorable
  Negative
end
## Un résumé.
struct Summary
  title: String ## Titre court
  bullets: Array(String)
  sentiment: Sentiment
end
## Résume un article.
prompt summarize(article: String) -> ~Summary using :fast
  system \"Sois concis.\"
  user \"Article : #{article}\"
end
";

fn summary_reply() -> Response {
    Response::json_reply(json!({"title": "Chat", "bullets": ["dort"], "sentiment": "Positive"}))
}

#[test]
fn prompt_builds_a_structured_request_and_returns_a_tainted_value() {
    let src = format!(
        "{SUMMARY}s = summarize(\"Le chat dort.\")\nputs s.title\np s.tainted?, s.title.tainted?\nputs s.sentiment\np s.trust!.tainted?\n"
    );
    let r = run_with(&src, vec![summary_reply()], &[]);
    r.result.unwrap();
    assert_eq!(r.output, "Chat\ntrue\ntrue\nPositive\nfalse\n");

    let request = &r.requests[0];
    assert_eq!(request["model"], "claude-haiku-4-5");
    assert_eq!(request["temperature"], 0.2);
    assert_eq!(request["system"], "Résume un article.\n\nSois concis.");
    assert_eq!(request["messages"][0], json!({"role": "user", "content": "Article : Le chat dort."}));
    let schema = &request["output_config"]["format"]["schema"];
    assert_eq!(schema["description"], "Un résumé.");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["title", "bullets", "sentiment"]));
    assert_eq!(schema["properties"]["title"]["description"], "Titre court");
    assert_eq!(schema["properties"]["sentiment"]["enum"], json!(["Positive", "Negative"]));
    assert_eq!(schema["properties"]["sentiment"]["description"], "Positive : Ton favorable");
}

#[test]
fn scalar_prompt_outputs_are_wrapped() {
    let src = "model :m, name: \"claude-haiku-4-5\"\nprompt count(t: String) -> Int\n  user t\nend\np count(\"x\")\n";
    let r = run_with(src, vec![Response::json_reply(json!({"value": 7}))], &[]);
    r.result.unwrap();
    assert_eq!(r.output, "~7\n");
    assert_eq!(r.requests[0]["output_config"]["format"]["schema"]["properties"]["value"]["type"], "integer");
}

#[test]
fn invalid_output_is_retried_once() {
    let src = format!("{SUMMARY}puts summarize(\"x\").title\n");
    let r = run_with(&src, vec![Response::text_reply("pas du JSON"), summary_reply()], &[]);
    r.result.unwrap();
    assert_eq!(r.output, "Chat\n");
    assert_eq!(r.requests.len(), 2);

    let e = run_err(&src, vec![Response::text_reply("{}"), Response::text_reply("{}")]);
    assert_eq!(e.ty, "LlmError");
    assert!(e.message.contains("champ `title` manquant"), "{}", e.message);
}

#[test]
fn refusal_raises() {
    let src = format!("{SUMMARY}summarize(\"x\")\n");
    let e = run_err(&src, vec![Response::from_content(vec![], "refusal")]);
    assert_eq!(e.ty, "LlmRefusal");
}

// ── Teinte ───────────────────────────────────────────────────

const MAILER: &str = "\
tool send(to: String, body: String) -> Unit uses net(\"smtp.example.com\")
  puts \"envoyé à #{to} : #{body}\"
end
";

#[test]
fn tainted_values_cannot_reach_dangerous_effects() {
    let src = format!("{SUMMARY}{MAILER}s = summarize(\"x\")\nsend(\"a@b.c\", \"Titre : #{{s.title}}\")\n");
    let e = run_err(&src, vec![summary_reply()]);
    assert_eq!(e.ty, "TaintError");
    assert!(e.message.contains("`send` (effet `net`)"), "{}", e.message);
}

#[test]
fn validated_values_can() {
    let src = format!(
        "{SUMMARY}{MAILER}s = summarize(\"x\").check {{ |x| x.bullets.size > 0 }}?\nsend(\"a@b.c\", s.title)\n"
    );
    let r = run_with(&src, vec![summary_reply()], &[]);
    r.result.unwrap();
    assert_eq!(r.output, "envoyé à a@b.c : Chat\n");
}

#[test]
fn failed_check_is_an_err_result() {
    let src = format!("{SUMMARY}r = summarize(\"x\").check {{ |x| x.bullets.size > 5 }}\np r.err?\nr?\n");
    let r = run_with(&src, vec![summary_reply()], &[]);
    assert_eq!(r.output, "true\n");
    assert_eq!(r.result.unwrap_err().ty, "CheckError");
}

#[test]
fn blocks_on_tainted_collections_see_tainted_items() {
    let src = format!("{SUMMARY}s = summarize(\"x\")\ns.bullets.each {{ |b| p b.tainted? }}\n");
    let r = run_with(&src, vec![summary_reply()], &[]);
    r.result.unwrap();
    assert_eq!(r.output, "true\n");
}

#[test]
fn human_approval_untaints_or_denies() {
    let src = format!("{SUMMARY}{MAILER}s = summarize(\"x\").approve(by: :human)\nsend(\"a@b.c\", s.title)\n");
    let approved = run_with(&src, vec![summary_reply()], &["o"]);
    approved.result.unwrap();
    assert!(approved.output.ends_with("envoyé à a@b.c : Chat\n"), "{}", approved.output);

    let denied = run_with(&src, vec![summary_reply()], &["n"]);
    assert_eq!(denied.result.unwrap_err().ty, "ApprovalDenied");
}

// ── Budgets ──────────────────────────────────────────────────

#[test]
fn budget_exceeded_is_raised_and_rescuable() {
    let src = format!(
        "{SUMMARY}within budget(usd: 0.001) do\n  summarize(\"x\")\n  puts \"pas atteint\"\nrescue BudgetExceeded => e\n  puts \"stop à #{{e.spent}}\"\nend\n"
    );
    // 1 000 tokens d'entrée + 1 000 de sortie sur Haiku 4.5 = 0,006 $
    let r = run_with(&src, vec![summary_reply().with_usage(1000, 1000)], &[]);
    let summary = r.result.unwrap();
    assert_eq!(r.output, "stop à $0.0060\n");
    assert_eq!(summary.llm_calls, 1);
    assert!((summary.cost_usd - 0.006).abs() < 1e-9);
}

// ── Agents ───────────────────────────────────────────────────

const READER: &str = "\
model :smart, provider: :anthropic, name: \"claude-opus-5\"
struct Report
  answer: String
  files: Array(String)
end
## Lit un fichier.
tool read_file(path: String, max_lines: Int = 10) -> String uses fs.read
  \"contenu de #{path} (#{max_lines} lignes)\"
end
agent Reader
  model :smart
  tools read_file
  max_turns 4
  instructions \"Tu lis des fichiers.\"
  @asked: Int = 0
  on Ask(question: String) -> ~Report
    @asked += 1
    run \"Question : #{question}\"
  end
  on Asked -> Int
    @asked
  end
end
";

#[test]
fn agent_runs_the_tool_loop_until_final_answer() {
    let src = format!(
        "{READER}r = spawn Reader\nrep = r.ask(Ask(question: \"que contient a.txt ?\"))\nputs rep.answer\np rep.files, r.ask(Asked())\n"
    );
    let replies = vec![
        Response::tool_call("t1", "read_file", json!({"path": "a.txt", "max_lines": null})),
        Response::tool_call("t2", "final_answer", json!({"answer": "du texte", "files": ["a.txt"]})),
    ];
    let r = run_with(&src, replies, &[]);
    r.result.unwrap();
    assert_eq!(r.output, "du texte\n~[\"a.txt\"]\n1\n");

    let requests = r.requests;
    assert_eq!(requests.len(), 2);
    let first = &requests[0];
    assert!(first["system"].as_str().unwrap().starts_with("Tu lis des fichiers."));
    assert_eq!(first["messages"][0]["content"], "Question : que contient a.txt ?");
    let tools = first["tools"].as_array().unwrap();
    assert_eq!(tools[0]["name"], "read_file");
    assert_eq!(tools[0]["description"], "Lit un fichier.");
    assert_eq!(tools[0]["strict"], true);
    assert_eq!(tools[0]["input_schema"]["properties"]["max_lines"]["anyOf"][1]["type"], "null");
    assert_eq!(tools[1]["name"], "final_answer");

    let second = &requests[1]["messages"];
    assert_eq!(second[1]["role"], "assistant");
    let result = &second[2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "t1");
    assert_eq!(result["content"], "contenu de a.txt (10 lignes)");
    assert_eq!(result["is_error"], false);
}

#[test]
fn tool_errors_are_reported_to_the_model() {
    let src = format!("{READER}r = spawn Reader\nputs r.ask(Ask(question: \"?\")).answer\n");
    let replies = vec![
        Response::tool_call("t1", "read_file", json!({"path": 42, "max_lines": null})),
        Response::tool_call("t2", "final_answer", json!({"answer": "ok", "files": []})),
    ];
    let r = run_with(&src, replies, &[]);
    r.result.unwrap();
    let result = &r.requests[1]["messages"][2]["content"][0];
    assert_eq!(result["is_error"], true);
    assert!(result["content"].as_str().unwrap().contains("chaîne attendue"), "{result}");
}

#[test]
fn agent_gives_up_after_max_turns() {
    let src = format!("{READER}r = spawn Reader\nr.ask(Ask(question: \"?\"))\n");
    let replies = (0..4).map(|_| Response::text_reply("je réfléchis")).collect();
    let e = run_err(&src, replies);
    assert_eq!(e.ty, "MaxTurnsExceeded");
}

#[test]
fn unknown_message_is_an_error() {
    let src = format!("{READER}r = spawn Reader\nr.ask(Asked(1))\n");
    let e = run_err(&src, vec![]);
    assert_eq!(e.ty, "ArgumentError");
}

// ── Tests Grenat ─────────────────────────────────────────────

#[test]
fn grenat_test_blocks() {
    let src = "\
test \"addition\" do
  assert_equal 4, 2 + 2
end
test \"échec\" do
  assert 1 > 2, \"un n'est pas plus grand que deux\"
end
test \"erreurs\" do
  assert_raises ZeroDivisionError { 1 / 0 }
end
";
    let parsed = grenat_parser::parse(src);
    let outcomes = run_tests(&parsed.program, Options::default()).unwrap();
    let summary: Vec<_> =
        outcomes.iter().map(|o| (o.name.as_str(), o.error.as_ref().map(|e| e.message.as_str()))).collect();
    assert_eq!(summary, [("addition", None), ("échec", Some("un n'est pas plus grand que deux")), ("erreurs", None)]);
}

// ── Exemples du dépôt ────────────────────────────────────────

#[test]
fn explorateur_example_runs_end_to_end() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/explorateur.grn")).unwrap();
    let replies = vec![
        Response::tool_call("t1", "list_dir", json!({"path": "src"})),
        Response::tool_call("t2", "read_file", json!({"path": "src/lib.rs", "max_lines": 1})),
        Response::tool_call(
            "t3",
            "final_answer",
            json!({"summary": "Un interpréteur.", "files_read": ["src/lib.rs"], "maturity": "Solide", "next_steps": ["Typer"]}),
        ),
        Response::text_reply("Grenat, enfin des agents typés."),
    ];
    let r = run_with(&src, replies, &[]);
    r.result.unwrap();
    assert_eq!(
        r.output,
        "## arg1 — Solide\nUn interpréteur.\n\nFichiers lus : src/lib.rs\n- Typer\n\n> Grenat, enfin des agents typés.\n"
    );
    let listing = &r.requests[1]["messages"][2]["content"][0]["content"];
    assert!(listing.as_str().unwrap().contains("builtins.rs"), "{listing}");
    let first_line = &r.requests[2]["messages"][4]["content"][0]["content"];
    assert_eq!(first_line, "//! Interpréteur de Grenat (phase 1) : exécution directe de l'AST.");
    assert_eq!(r.requests[0]["fallbacks"], "default");
    assert_eq!(r.requests[3]["model"], "claude-haiku-4-5");
}

#[test]
fn bases_example_runs() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/bases.grn")).unwrap();
    assert!(run(&src).starts_with("fib(25) = 75025\n"));
}

#[test]
fn support_desk_example_runs_end_to_end() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/support_desk.grn")).unwrap();
    let triage = |category: &str| {
        Response::json_reply(json!({"category": category, "priority": "Normal", "language": "fr", "reason": "motif"}))
    };
    let replies = vec![
        triage("Question"),
        Response::tool_call(
            "t1",
            "final_answer",
            json!({"body": "Menu Factures > Exporter.", "sources": ["docs/export.md"], "confidence": 0.9}),
        ),
        triage("Spam"),
    ];
    let r = run_full(&src, replies, &["o"], &["../../examples/tickets.jsonl"]);
    if let Err(e) = &r.result {
        panic!("{e:?}\n{}", r.output);
    }
    assert!(r.output.contains("Envoyer à ana@example.com ?"), "{}", r.output);
    assert!(
        r.output.contains("[smtp] à ana@example.com — Re: votre demande\nMenu Factures > Exporter.\n"),
        "{}",
        r.output
    );
    assert!(r.output.ends_with("✓ 1 envoyés, 1 ignorés — coût $0.0014\n"), "{}", r.output);
    assert_eq!(r.requests.len(), 3);
    assert_eq!(r.requests[1]["model"], "claude-opus-5");
}

// ── Phase 2 : exécution alignée sur le vérificateur ──────────

#[test]
fn methods_on_tainted_values_keep_self_tainted() {
    let src = format!(
        "{SUMMARY}{MAILER}struct Box2\n  inner: Summary\n  def mail = send(\"a\", inner.title)\nend\nb = Box2(inner: summarize(\"x\"))\nb.mail\n"
    );
    let e = run_err(&src, vec![summary_reply()]);
    assert_eq!(e.ty, "TaintError");
}

#[test]
fn validation_methods_accept_clean_values() {
    let src = "p \"a\".trust!, [1].check { |x| x.size > 0 }.ok?\n";
    assert_eq!(run(src), "\"a\"\ntrue\n");
}

#[test]
fn filesystem_capabilities_are_enforced() {
    let src = "def load(path: String) -> String uses fs.read(\"./src\")\n  File.read(path)\nend\nputs load(\"src/lib.rs\").lines.first\nload(\"Cargo.toml\")\n";
    let r = run_with(src, vec![], &[]);
    assert!(r.output.starts_with("//! Interpréteur"), "{}", r.output);
    let e = r.result.unwrap_err();
    assert_eq!(e.ty, "CapabilityError");
    assert!(e.message.contains("`load` (uses fs.read(\"./src\"))"), "{}", e.message);

    let escape = "def load(path: String) -> String uses fs.read(\"./src\")\n  File.read(path)\nend\nload(\"src/../Cargo.toml\")\n";
    assert_eq!(run_err(escape, vec![]).ty, "CapabilityError");
}
