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
