//! What the runtime records for `grenat console`: model calls and whom they
//! were made for, failures and refusals, eval runs.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Response, Scripted, run_evals};
use grenat_ops::{calls, evals, events};
use serde_json::json;

/// A new SQLite database file, and its URL.
fn database(name: &str) -> (std::path::PathBuf, String) {
    let path = temp_dir(name).join("app.db");
    let url = format!("sqlite://{}", path.display());
    (path, url)
}

fn open(url: &str) -> Box<dyn grenat_db::Connection> {
    grenat_db::connect(url).unwrap()
}

#[test]
fn every_model_call_is_recorded_with_its_agent_workflow_and_job() {
    let (_, url) = database("calls");
    let src = format!(
        "database \"{url}\"
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt echo(t: String) -> ~String using :fast
  user t
end
agent Helper
  model :fast
  on Help(text: String) -> ~String
    run text
  end
end
workflow flow(n: Int) -> ~String uses llm
  step(:a) {{ echo(\"x\") }}
end
echo(\"top\")
spawn(Helper).ask(Help(text: \"hi\"))
flow(1)
enqueue(:flow, 2)
Jobs.perform
"
    );
    let value = || Response::json_reply(json!({"value": "ok"}));
    let replies = vec![value(), Response::tool_call("t1", "final_answer", json!({"value": "done"})), value(), value()];
    run_full(&src, replies, &[], &[]).ok();
    let recorded = calls::since(open(&url).as_mut(), 0.0).unwrap();
    let whom: Vec<_> = recorded.iter().map(|c| (c.agent.as_deref(), c.workflow.as_deref(), c.job_id)).collect();
    assert_eq!(
        whom,
        [(None, None, None), (Some("Helper"), None, None), (None, Some("flow"), None), (None, Some("flow"), Some(1))]
    );
    // the scripted model has no price: the model asked for is billed
    assert!(recorded.iter().all(|c| c.model == "claude-haiku-4-5" && c.cost_usd > 0.0 && c.input_tokens == 100));
}

#[test]
fn failed_jobs_are_events_and_refusals_are_told_apart() {
    let (_, url) = database("events");
    let src = format!(
        "database \"{url}\"
def boom(n: Int) uses db
  raise ArgumentError, \"no #{{n}}\"
end
def peek uses db
  File.read(\"/etc/hosts\")
end
enqueue(:boom, 1)
enqueue(:peek)
Jobs.perform
"
    );
    run_full(&src, Vec::new(), &[], &[]).ok();
    let mut db = open(&url);
    let all = events::latest(db.as_mut(), false, 100).unwrap();
    // three runs of each job before it is given up
    assert_eq!(all.len(), 6);
    assert!(
        all.iter()
            .any(|e| (e.source.as_str(), e.subject.as_str(), e.error.as_str())
                == ("job", "job 1 (boom)", "ArgumentError"))
    );
    let refusals = events::latest(db.as_mut(), true, 100).unwrap();
    assert_eq!(refusals.len(), 3);
    assert!(refusals.iter().all(|e| e.error == "CapabilityError" && e.subject == "job 2 (peek)"), "{refusals:?}");
}

#[test]
fn eval_runs_are_kept() {
    let (_, url) = database("evals");
    let dir = temp_dir("eval-dataset");
    std::fs::write(dir.join("rows.jsonl"), "{\"input\": \"a\"}\n{\"input\": \"b\"}\n").unwrap();
    let src = format!(
        "database \"{url}\"
eval \"is a\", dataset: \"rows.jsonl\", threshold: 0.4 do |row|
  row.input == \"a\"
end
"
    );
    let parsed = grenat_parser::parse(&src);
    assert!(parsed.diagnostics.is_empty());
    let options = Options {
        provider: Some(Arc::new(Scripted::new(Vec::new()))),
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        dir: Some(dir),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    let reports = run_evals(&parsed.program, options, None).unwrap();
    assert!(reports[0].passed());
    let runs = evals::history(open(&url).as_mut()).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].name.as_str(), runs[0].score, runs[0].rows, runs[0].passed), ("is a", 0.5, 2, true));
}

#[test]
fn without_a_database_nothing_is_recorded_and_nothing_fails() {
    let src = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt echo(t: String) -> ~String using :fast
  user t
end
puts echo(\"hi\").trust!
";
    let out = run_full(src, vec![Response::text_reply("ok")], &[], &[]).ok();
    assert_eq!(out, "ok\n");
}
