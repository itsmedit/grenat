//! Evals: datasets scored row by row, `judge`, reports.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::*;
use grenat_interp::{EvalReport, Options, Output, Response, Scripted, run_evals};
use serde_json::json;

const MODEL: &str = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\n";

fn evals_in(src: &str, dir: &Path, provider: Scripted, filter: Option<&str>) -> (Vec<EvalReport>, Arc<Scripted>) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let provider = Arc::new(provider);
    let options = Options {
        provider: Some(provider.clone()),
        output: Output::Capture(Default::default()),
        journal: Some(temp_dir("journal")),
        dir: Some(dir.to_path_buf()),
        ..Options::default()
    };
    (run_evals(&parsed.program, options, filter).unwrap(), provider)
}

fn dataset(dir: &Path, name: &str, rows: &[serde_json::Value]) {
    let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
    std::fs::write(dir.join(name), text).unwrap();
}

/// A judge scoring 1 when the material mentions cats.
fn cat_judge() -> Scripted {
    Scripted::responder(|body| {
        let content = body["messages"][0]["content"].as_str().unwrap_or_default();
        let material = content.split_once("<material").map_or("", |(_, m)| m);
        let score = if material.contains("cat") { 1.0 } else { 0.0 };
        Response::json_reply(json!({"reasoning": "checked", "score": score}))
    })
}

#[test]
fn an_eval_scores_every_row_with_a_judge() {
    let dir = temp_dir("judge");
    dataset(
        &dir,
        "rows.jsonl",
        &[json!({"text": "a cat"}), json!({"text": "a dog"}), json!({"text": "cats!"}), json!({"text": "boom"})],
    );
    let src = format!(
        "{MODEL}\
eval \"about cats\", dataset: \"rows.jsonl\", threshold: 0.5 do |row|
  raise \"no text\" if row.text == \"boom\"
  judge(:fast, \"Is it about cats?\", row.text)
end
"
    );
    let (reports, provider) = evals_in(&src, &dir, cat_judge(), None);
    let report = &reports[0];
    assert_eq!(report.name, "about cats");
    let scores: Vec<f64> = report.rows.iter().map(|r| r.score).collect();
    assert_eq!(scores, [1.0, 0.0, 1.0, 0.0]);
    assert_eq!(report.mean(), 0.5);
    assert!(report.passed());
    let failed = report.rows[3].error.as_ref().unwrap();
    assert_eq!((failed.ty.as_str(), failed.message.as_str()), ("RuntimeError", "no text"));
    assert!(report.rows[..3].iter().all(|r| r.error.is_none() && r.cost_usd > 0.0));
    assert!(report.cost_usd() > 0.0);

    let requests = provider.requests();
    assert_eq!(requests.len(), 3);
    let request = &requests[0];
    assert!(request["system"].as_str().unwrap().contains("impartial evaluator"));
    assert_eq!(request["output_config"]["format"]["schema"]["required"], json!(["reasoning", "score"]));
    let content = request["messages"][0]["content"].as_str().unwrap();
    assert!(content.starts_with("Question: Is it about cats?\n\n<material index=\"1\">\n"), "{content}");
}

#[test]
fn scores_are_booleans_or_fractions() {
    let dir = temp_dir("scores");
    dataset(&dir, "n.jsonl", &[json!({"n": 1}), json!({"n": 2}), json!({"n": 3}), json!({"n": 4})]);
    let src = "\
eval \"booleans\", dataset: \"n.jsonl\", threshold: 0.75 do |row|
  row.n > 1
end
eval \"fractions\", dataset: \"n.jsonl\" do |row|
  row.n / 4.0
end
eval \"invalid\", dataset: \"n.jsonl\" do |row|
  \"yes\"
end
";
    let (reports, _) = evals_in(src, &dir, Scripted::new([]), None);
    assert_eq!(reports[0].mean(), 0.75);
    assert!(reports[0].passed());
    assert_eq!(reports[1].mean(), 0.625);
    // the default threshold is a perfect score
    assert!(!reports[1].passed());
    let error = reports[2].rows[0].error.as_ref().unwrap();
    assert_eq!(error.message, "a score is a Bool or a Float from 0 to 1, got \"yes\"");
    assert!(!reports[2].passed());
}

#[test]
fn evals_are_selected_by_name_and_report_bad_datasets() {
    let dir = temp_dir("select");
    std::fs::write(dir.join("bad.jsonl"), "{\"a\": 1}\n[1, 2]\n").unwrap();
    let src = "\
eval \"missing\", dataset: \"nowhere.jsonl\" do |row|
  true
end
eval \"not objects\", dataset: \"bad.jsonl\" do |row|
  true
end
";
    let (reports, _) = evals_in(src, &dir, Scripted::new([]), None);
    let missing = reports[0].error.as_ref().unwrap();
    assert_eq!(missing.ty, "IOError");
    assert!(missing.message.starts_with("cannot read the dataset"), "{}", missing.message);
    let bad = reports[1].error.as_ref().unwrap();
    assert!(bad.message.starts_with("a row must be a JSON object, at "), "{}", bad.message);
    assert!(bad.message.ends_with("bad.jsonl:2"), "{}", bad.message);
    assert!(!reports[0].passed() && !reports[1].passed());

    let (reports, _) = evals_in(src, &dir, Scripted::new([]), Some("objects"));
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].name, "not objects");
}

#[test]
fn eval_options_are_checked() {
    let r = run_err("eval \"x\", dataset: \"d.jsonl\", threshold: 2 do |row|\n  true\nend\n", Vec::new());
    assert_eq!(r.message, "invalid eval option `threshold: 2`");
    let r = run_err("eval \"x\" do |row|\n  true\nend\n", Vec::new());
    assert!(r.message.starts_with("`eval` expects a dataset"), "{}", r.message);
    // `grenat run` only declares them
    assert_eq!(run("eval \"x\", dataset: \"d.jsonl\" do |row|\n  raise \"never\"\nend\nputs 1\n"), "1\n");
}

#[test]
fn a_judge_must_give_a_score_from_0_to_1() {
    let src = format!("{MODEL}def main uses llm\n  p judge(\"Good?\", \"text\")\nend\n");
    let r = run_with(&src, vec![Response::json_reply(json!({"reasoning": "fine", "score": 0.8}))], &[]);
    assert_eq!(r.ok(), "0.8\n");
    let e = run_err(&src, vec![Response::json_reply(json!({"reasoning": "?", "score": 7}))]);
    assert!(e.message.starts_with("the judge gave no score from 0 to 1"), "{}", e.message);
}
