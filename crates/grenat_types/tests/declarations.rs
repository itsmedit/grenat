//! Declarations: agents, prompts, supervisors (E0100, E0500).

mod common;

use common::*;

#[test]
fn agents_are_checked() {
    let agent = "model :m, name: \"claude-haiku-4-5\"\ntool look(q: String) -> String uses net\n  q\nend\nagent A\n  model :m\n  tools look\n  on Go(topic: String) -> ~String\n    run \"cherche #{topic}\"\n  end\nend\n";
    clean(&format!("{agent}def main uses llm, net\n  spawn(A).ask(Go(topic: \"x\"))\nend\n"));
    single(&format!("{agent}def main uses llm, net\n  spawn(A).ask(Stop())\nend\n"), "E0100", "Stop()");
    single(&agent.replace("tools look", "tools lok"), "E0100", "lok");
    single(&agent.replace("model :m\n  tools", "model :n\n  tools"), "E0100", ":n");
    single(
        &format!("{agent}def main uses llm\n  spawn(A).ask(Go(topic: \"x\"))\nend\n"),
        "E0300",
        "spawn(A).ask(Go(topic: \"x\"))",
    );
}

#[test]
fn prompt_output_must_be_serializable() {
    let src = "model :m, name: \"claude-haiku-4-5\"\nprompt p(x: String) -> ~Hash(String, Int)\n  user x\nend\n";
    single(src, "E0500", "~Hash(String, Int)");
}

// ── Phase 3: concurrency and supervision ─────────────────────

const DESK: &str = "\
agent Worker
  on Work -> Int
    1
  end
end
";

#[test]
fn concurrency_constructs_are_typed() {
    clean(&format!(
        "{DESK}pool = spawn_pool(Worker, size: 3)\nxs = [1, 2].parallel_map(limit: 2) {{ |x| pool.ask(Work()) + x }}\nputs xs.sum\nr = race do\n  1\n  2\nend\n"
    ));
}

#[test]
fn runtime_errors_of_actors_can_be_rescued() {
    clean(&format!(
        "{DESK}begin\n  spawn(Worker).ask(Work())\nrescue DeadlockError, AgentDown, Cancelled => e\n  puts e.message\nend\n"
    ));
}

#[test]
fn supervisor_options_are_checked() {
    let valid =
        format!("{DESK}supervisor Desk, strategy: :rest_for_one, max_restarts: 2, within: 30.s\n  child Worker\nend\n");
    clean(&valid);

    let d = single(&valid.replace(":rest_for_one", ":one_for_al"), "E0500", ":one_for_al");
    assert_eq!(d.help.as_deref(), Some("did you mean `one_for_all`?"));
    single(&valid.replace("max_restarts: 2", "max_restarts: \"deux\""), "E0200", "\"deux\"");
    single(&valid.replace("within: 30.s", "within: \"30s\""), "E0200", "\"30s\"");
    let d = single(&valid.replace("max_restarts:", "max_restart:"), "E0500", "max_restart:");
    assert_eq!(d.help.as_deref(), Some("did you mean `max_restarts`?"));
}

#[test]
fn supervisor_children_must_be_agents() {
    single(&format!("{DESK}supervisor Desk\n  child Wroker\nend\n"), "E0100", "Wroker");
    single(&format!("{DESK}supervisor Desk\n  child Worker\nend\nDesk[Other]\n"), "E0100", "Other");
}

#[test]
fn models_are_declared_once_from_a_known_provider() {
    let src = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\nmodel :fast, provider: :openai, name: \"gpt-5\"\n";
    let d = single(src, "E0100", ":fast");
    assert!(d.message.contains("declared twice"), "{}", d.message);
    let d = single("model :x, provider: :opanai, name: \"gpt-5\"\n", "E0500", ":opanai");
    assert_eq!(d.help.as_deref(), Some("did you mean `openai`?"));
    clean(
        "model :x, provider: :ollama, name: \"llama3.3\", base_url: \"http://gpu:11434/v1\", price: {input: 0, output: 0}\n",
    );
}

#[test]
fn prompt_caching_is_an_option_of_the_model() {
    clean("model :a, provider: :anthropic, name: \"claude-opus-5\", cache: true, cache_ttl: \"1h\"\n");
    clean("model :a, provider: :anthropic, name: \"claude-opus-5\", cache: :agents, cache_ttl: \"5m\"\n");
    clean(
        "model :a, provider: :anthropic, name: \"claude-opus-5\", cache: false, price: {input: 5, output: 25, cache_read: 0.5, cache_write: 6.25}\n",
    );
    let d = single("model :a, provider: :anthropic, name: \"claude-opus-5\", cache: :always\n", "E0500", ":always");
    assert!(d.message.contains("`true`, `false` or `:agents`"), "{}", d.message);
    single("model :a, provider: :anthropic, name: \"claude-opus-5\", cache: \"yes\"\n", "E0500", "\"yes\"");
    let d = single("model :a, provider: :anthropic, name: \"claude-opus-5\", cache_ttl: \"2h\"\n", "E0500", "\"2h\"");
    assert!(d.message.contains("\"1h\""), "{}", d.message);
    let d = single("model :a, provider: :anthropic, name: \"claude-opus-5\", cach: true\n", "E0500", "cach:");
    assert_eq!(d.help.as_deref(), Some("did you mean `cache`?"));
}

/// What the interpreter refuses when the program loads (`program.rs`),
/// the checker refuses where it is written as a literal.
const REFUSED_MODEL_OPTIONS: [&str; 11] = [
    "cache_ttl: :hour",
    "cache_ttl: 3600",
    "cache: nil",
    "cache: \"agents\"",
    "kind: :embedding, dimensions: 0",
    "kind: :embedding, dimensions: \"3\"",
    "kind: :embedding, dimensions: 3.5",
    "kind: :transcription, price: {minute: -1}",
    "kind: :transcription, price: {minute: \"x\"}",
    "price: {input: -1, output: 2}",
    "max_tokens: 0",
];

#[test]
fn model_options_are_checked_as_the_program_loads_them() {
    for options in REFUSED_MODEL_OPTIONS {
        let src = format!("model :m, provider: :openai, name: \"whisper-1\", {options}\n");
        let found = diags(&src);
        assert!(found.iter().any(|d| d.code == Some("E0500")), "{options}: {found:#?}");
    }
    // computed values are the interpreter's to check
    clean("RATE = 2\nmodel :m, provider: :openai, name: \"gpt-5\", price: {input: RATE, output: 8}\n");
    // a kind written as a string is a kind
    clean(
        "model :e, provider: :openai, name: \"text-embedding-3-small\", kind: \"embedding\"\ndef f -> Array(Float) uses llm = embed(:e, \"x\")\n",
    );
}
