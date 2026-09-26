//! `prompt`: structured request, tainted output, validation and retry.

mod common;

use common::*;
use grenat_interp::Response;
use serde_json::json;

#[test]
fn prompt_builds_a_structured_request_and_returns_a_tainted_value() {
    let src = format!(
        "{SUMMARY}s = summarize(\"The cat sleeps.\")\nputs s.title\np s.tainted?, s.title.tainted?\nputs s.sentiment\np s.trust!.tainted?\n"
    );
    let r = run_with(&src, vec![summary_reply()], &[]);
    r.result.unwrap();
    assert_eq!(r.output, "Cat\ntrue\ntrue\nPositive\nfalse\n");

    let request = &r.requests[0];
    assert_eq!(request["model"], "claude-haiku-4-5");
    assert_eq!(request["temperature"], 0.2);
    assert_eq!(request["system"], "Summarizes an article.\n\nBe concise.");
    assert_eq!(request["messages"][0], json!({"role": "user", "content": "Article: The cat sleeps."}));
    let schema = &request["output_config"]["format"]["schema"];
    assert_eq!(schema["description"], "A summary.");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["title", "bullets", "sentiment"]));
    assert_eq!(schema["properties"]["title"]["description"], "Short title");
    assert_eq!(schema["properties"]["sentiment"]["enum"], json!(["Positive", "Negative"]));
    assert_eq!(schema["properties"]["sentiment"]["description"], "Positive: Favourable tone");
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
    let r = run_with(&src, vec![Response::text_reply("not JSON"), summary_reply()], &[]);
    r.result.unwrap();
    assert_eq!(r.output, "Cat\n");
    assert_eq!(r.requests.len(), 2);

    let e = run_err(&src, vec![Response::text_reply("{}"), Response::text_reply("{}")]);
    assert_eq!(e.ty, "LlmError");
    assert!(e.message.contains("missing field `title`"), "{}", e.message);
}

#[test]
fn refusal_raises() {
    let src = format!("{SUMMARY}summarize(\"x\")\n");
    let e = run_err(&src, vec![Response::from_content(vec![], "refusal")]);
    assert_eq!(e.ty, "LlmRefusal");
}

#[test]
fn a_model_without_a_known_price_is_said_once_and_priced_when_told() {
    let src = "model :gpt, provider: :openai, name: \"gpt-5\"
model :priced, provider: :openai, name: \"gpt-5-mini\", price: {input: 1.0, output: 2.0}
prompt a(t: String) -> ~String using :gpt
  user t
end
prompt b(t: String) -> ~String using :priced
  user t
end
a(\"x\")
a(\"y\")
b(\"z\")
";
    let replies = vec![Response::text_reply("1"), Response::text_reply("2"), Response::text_reply("3").with_usage(1_000_000, 0)];
    let run = run_full(src, replies, &[], &[]);
    let summary = run.result.as_ref().unwrap().clone();
    assert_eq!(run.output.matches("the price of `gpt-5` is unknown").count(), 1, "{}", run.output);
    assert!((summary.cost_usd - 1.0).abs() < 1e-9, "{}", summary.cost_usd);
}
