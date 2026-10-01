//! Prompt caching: which requests carry breakpoints (`cache:`,
//! `cache_ttl:`), what the cache did in the ledger, the log and the cost.

mod common;

use common::*;
use grenat_interp::{Attempt, Response, Scripted};
use grenat_ops::calls;
use serde_json::{Value as Json, json};

const MODELS: &str = "\
model :fast, provider: :anthropic, name: \"claude-opus-5\"
model :plain, provider: :anthropic, name: \"claude-opus-5\", cache: false
model :all, provider: :anthropic, name: \"claude-opus-5\", cache: true, cache_ttl: \"1h\"
";

/// The breakpoints of a request body.
fn marks(body: &Json) -> usize {
    body.to_string().matches("\"cache_control\"").count()
}

/// An agent answering at once, with the model `model`.
fn agent(name: &str, model: &str) -> String {
    format!(
        "agent {name}
  model :{model}
  instructions \"You help.\"
  on Help(text: String) -> ~String
    run text
  end
end
"
    )
}

#[test]
fn agents_are_cached_by_default_prompts_when_asked() {
    let src = format!(
        "{MODELS}{}{}
prompt plain_prompt(t: String) -> ~String using :fast
  system \"Be brief.\"
  user t
end
prompt cached_prompt(t: String) -> ~String using :all
  system \"Be brief.\"
  user t
end
spawn(Helper).ask(Help(text: \"a\"))
spawn(Plain).ask(Help(text: \"b\"))
plain_prompt(\"c\")
cached_prompt(\"d\")
",
        agent("Helper", "fast"),
        agent("Plain", "plain")
    );
    let done = || Response::tool_call("t", "final_answer", json!({"value": "ok"}));
    let replies = vec![done(), done(), Response::text_reply("c"), Response::text_reply("d")];
    let r = run_with(&src, replies, &[]);
    r.result.unwrap();
    let five = json!({"type": "ephemeral"});
    let helper = &r.requests[0];
    assert_eq!(helper["tools"][0]["cache_control"], five);
    assert_eq!(helper["system"][0]["cache_control"], five);
    assert_eq!(helper["messages"][0]["content"][0]["cache_control"], five);
    assert_eq!(marks(helper), 3);
    // `cache: false`
    assert_eq!((marks(&r.requests[1]), r.requests[1]["system"].is_string()), (0, true));
    // a prompt, by default
    assert_eq!((marks(&r.requests[2]), r.requests[2]["system"].as_str()), (0, Some("Be brief.")));
    // `cache: true, cache_ttl: "1h"`: its system prompt, not its question
    let cached = &r.requests[3];
    assert_eq!(cached["system"][0]["cache_control"], json!({"type": "ephemeral", "ttl": "1h"}));
    assert_eq!((marks(cached), cached["messages"][0]["content"].as_str()), (1, Some("d")));
}

#[test]
fn what_the_cache_served_is_recorded_logged_and_priced() {
    let path = temp_dir("cache-ledger").join("app.db");
    let url = format!("sqlite://{}", path.display());
    let src = format!(
        "database \"{url}\"
{MODELS}model :given, provider: :openai, name: \"gpt-9\", price: {{input: 2, output: 8, cache_read: 0.5}}
prompt ask(t: String) -> ~String using :fast
  user t
end
prompt elsewhere(t: String) -> ~String using :given
  user t
end
ask(\"a\")
elsewhere(\"b\")
"
    );
    let mut reply = Response::text_reply("ok");
    reply.model = "claude-opus-5".into();
    reply.usage.input_tokens = 100;
    reply.usage.cache_read_input_tokens = 9_000;
    reply.usage.cache_creation_input_tokens = 1_000;
    reply.usage.output_tokens = 20;
    let mut other = reply.clone();
    other.usage.cache_creation_input_tokens = 0;
    let mode = Mode { log: true, ..Mode::default() };
    let r = run_mode(&src, Scripted::new([reply, other]), &[], &[], mode);
    r.result.unwrap();
    assert!(r.output.contains("[llm] claude-opus-5 · 10100 in (9000 cached, 1000 to cache) / 20 out"), "{}", r.output);

    let recorded = calls::since(grenat_db::connect(&url).unwrap().as_mut(), 0.0).unwrap();
    let call = &recorded[0];
    assert_eq!((call.input_tokens, call.cached_tokens, call.cache_write_tokens), (10_100, 9_000, 1_000));
    // Opus 5: $5 a million at full price, $0.50 read, $6.25 written, $25 out
    let expected = (100.0 * 5.0 + 9_000.0 * 0.5 + 1_000.0 * 6.25 + 20.0 * 25.0) / 1_000_000.0;
    assert!((call.cost_usd - expected).abs() < 1e-12, "{} != {expected}", call.cost_usd);
    // a price given, its cache reads too
    let given = &recorded[1];
    let expected = (100.0 * 2.0 + 9_000.0 * 0.5 + 20.0 * 8.0) / 1_000_000.0;
    assert!((given.cost_usd - expected).abs() < 1e-12, "{} != {expected}", given.cost_usd);
    assert!((calls::cached_share(&recorded) - 18_000.0 / 19_200.0).abs() < 1e-12);
}

#[test]
fn an_attempt_declined_after_writing_is_counted_recorded_and_priced() {
    let path = temp_dir("declined-ledger").join("app.db");
    let url = format!("sqlite://{}", path.display());
    let src = format!(
        "database \"{url}\"
{MODELS}prompt ask(t: String) -> ~String using :fast
  user t
end
within budget(usd: 0.04) do
  ask(\"a\")
end
"
    );
    // Opus 5 declined after 2,000 tokens; Opus 4.8 answered
    let mut reply = Response::text_reply("ok");
    reply.model = "claude-opus-4-8".into();
    reply.usage.input_tokens = 1_000;
    reply.usage.output_tokens = 100;
    let mut declined = reply.usage;
    declined.output_tokens = 2_000;
    reply.declined = vec![Attempt { model: "claude-opus-5".into(), usage: declined }];
    let mode = Mode { log: true, ..Mode::default() };
    let r = run_mode(&src, Scripted::new([reply]), &[], &[], mode);
    // $0.0075 answered, $0.055 declined: over the budget
    let e = r.result.unwrap_err();
    assert_eq!(e.ty, "BudgetExceeded", "{}", e.message);
    assert!(r.output.contains("(1 declined attempt(s) included)"), "{}", r.output);
    let recorded = calls::since(grenat_db::connect(&url).unwrap().as_mut(), 0.0).unwrap();
    let rows: Vec<(&str, i64, i64)> =
        recorded.iter().map(|c| (c.model.as_str(), c.input_tokens, c.output_tokens)).collect();
    assert_eq!(rows, [("claude-opus-4-8", 1_000, 100), ("claude-opus-5", 1_000, 2_000)]);
    // each at its own model's rates: Opus 4.8 $5 / $25, Opus 5 $5 / $25 a million
    let expected = [(1_000.0 * 5.0 + 100.0 * 25.0) / 1e6, (1_000.0 * 5.0 + 2_000.0 * 25.0) / 1e6];
    for (call, expected) in recorded.iter().zip(expected) {
        assert!((call.cost_usd - expected).abs() < 1e-12, "{} != {expected}", call.cost_usd);
    }
}

#[test]
fn cache_options_are_checked_when_the_program_loads() {
    let e = run_err("model :m, provider: :anthropic, name: \"claude-opus-5\", cache: :sometimes\nputs 1\n", Vec::new());
    assert!(e.message.contains("invalid option `cache: :sometimes`"), "{}", e.message);
    let e = run_err("model :m, provider: :anthropic, name: \"claude-opus-5\", cache_ttl: \"2h\"\nputs 1\n", Vec::new());
    assert!(e.message.contains("invalid option `cache_ttl: \"2h\"`"), "{}", e.message);
    let e = run_err(
        "model :m, provider: :openai, name: \"x\", price: {input: 1, output: 2, cache_read: \"cheap\"}\nputs 1\n",
        Vec::new(),
    );
    assert!(e.message.contains("`cache_read: …`"), "{}", e.message);
}
