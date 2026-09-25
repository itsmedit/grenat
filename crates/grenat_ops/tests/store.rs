//! The operations store on SQLite, and on PostgreSQL when
//! `GRENAT_TEST_POSTGRES` is a database URL (its tables are then temporary:
//! `pg_temp` comes first in the search path).

use grenat_db::{Connection, connect};
use grenat_ops::approvals::{self, Decision};
use grenat_ops::calls::{self, By, Call};
use grenat_ops::evals::{self, Run};
use grenat_ops::events;
use grenat_ops::jobs::{self, Status};
use grenat_ops::journal;
use serde_json::json;

/// `check` on a new SQLite database, then on PostgreSQL if there is one.
fn on_every_database(check: fn(&mut dyn Connection)) {
    check(connect("sqlite::memory:").unwrap().as_mut());
    if let Ok(url) = std::env::var("GRENAT_TEST_POSTGRES") {
        let mut db = connect(&url).unwrap();
        db.batch("SET search_path TO pg_temp").unwrap();
        check(db.as_mut());
    }
}

#[test]
fn a_job_is_queued_claimed_once_and_finished() {
    on_every_database(|db| {
        let id = jobs::enqueue(db, "triage", "[1]", 100.0, 90.0).unwrap();
        let later = jobs::enqueue(db, "digest", "[]", 500.0, 90.0).unwrap();
        assert!(jobs::next_due(db, 99.0).unwrap().is_none(), "not due yet");
        let job = jobs::next_due(db, 200.0).unwrap().unwrap();
        assert_eq!((job.id, job.name.as_str(), job.args.as_str(), job.status), (id, "triage", "[1]", Status::Queued));
        assert!(jobs::claim(db, id, 200.0).unwrap());
        assert!(!jobs::claim(db, id, 200.0).unwrap(), "a job is claimed once");
        jobs::finish(db, id, 1, 201.0).unwrap();
        let job = jobs::get(db, id).unwrap().unwrap();
        assert_eq!((job.status, job.attempts, job.created_at, job.updated_at), (Status::Done, 1, 90.0, 201.0));
        assert_eq!(jobs::queued(db).unwrap().iter().map(|j| j.id).collect::<Vec<_>>(), [later]);
        let counts = jobs::counts(db).unwrap();
        assert!(counts.contains(&(Status::Done, 1)) && counts.contains(&(Status::Queued, 1)) && counts.contains(&(Status::Failed, 0)));
        assert_eq!(jobs::list(db, None, 10).unwrap()[0].id, later, "newest first");
        assert!(jobs::get(db, 999).unwrap().is_none());
    });
}

#[test]
fn a_failed_job_is_retried_later_then_given_up_then_retried_by_hand() {
    on_every_database(|db| {
        let id = jobs::enqueue(db, "fetch", "[]", 0.0, 0.0).unwrap();
        jobs::claim(db, id, 1.0).unwrap();
        jobs::retry_later(db, id, 1, 61.0, "IoError: down", 1.0).unwrap();
        let job = jobs::get(db, id).unwrap().unwrap();
        assert_eq!((job.status, job.run_at, job.error.as_deref()), (Status::Queued, 61.0, Some("IoError: down")));
        assert!(!jobs::retry(db, id, 2.0).unwrap(), "only a failed job is retried by hand");
        jobs::claim(db, id, 61.0).unwrap();
        jobs::fail(db, id, 3, "IoError: still down", 62.0).unwrap();
        assert_eq!(jobs::list(db, Some(Status::Failed), 10).unwrap().len(), 1);
        assert!(jobs::retry(db, id, 100.0).unwrap());
        let job = jobs::get(db, id).unwrap().unwrap();
        assert_eq!((job.status, job.attempts, job.run_at), (Status::Queued, 0, 100.0));
    });
}

#[test]
fn an_approval_waits_and_its_decision_resumes_the_job() {
    on_every_database(|db| {
        let job = jobs::enqueue(db, "publish", "[7]", 0.0, 0.0).unwrap();
        jobs::claim(db, job, 1.0).unwrap();
        assert_eq!(approvals::decision(db, job, 0).unwrap(), None);
        approvals::ask(db, job, 0, "Publish?", 2.0).unwrap();
        jobs::wait(db, job, 2.0).unwrap();
        assert_eq!(approvals::decision(db, job, 0).unwrap(), Some(Decision::Pending));
        let pending = approvals::pending(db).unwrap();
        assert_eq!((pending.len(), pending[0].message.as_str(), pending[0].job_id), (1, "Publish?", job));
        assert!(approvals::decide(db, pending[0].id, true, 3.0).unwrap());
        assert!(!approvals::decide(db, pending[0].id, false, 4.0).unwrap(), "decided once");
        assert_eq!(approvals::decision(db, job, 0).unwrap(), Some(Decision::Approved));
        assert_eq!(jobs::get(db, job).unwrap().unwrap().status, Status::Queued, "the job runs again");
        let decided = approvals::decided(db, 10).unwrap();
        assert_eq!((decided[0].decision, decided[0].decided_at), (Decision::Approved, Some(3.0)));
        assert!(approvals::pending(db).unwrap().is_empty());
        assert_eq!(approvals::of_job(db, job).unwrap().len(), 1);
    });
}

#[test]
fn model_calls_are_summed_by_agent_workflow_model_and_day() {
    on_every_database(|db| {
        let call = |at: f64, model: &str, agent: Option<&str>, cost: f64| Call {
            at,
            model: model.into(),
            agent: agent.map(Into::into),
            workflow: Some("triage".into()),
            job_id: Some(3),
            input_tokens: 100,
            output_tokens: 10,
            cost_usd: cost,
        };
        calls::record(db, &call(10.0, "claude-haiku-4-5", Some("Triage"), 0.25)).unwrap();
        calls::record(db, &call(86_400.0 + 5.0, "claude-opus-5", Some("Writer"), 1.5)).unwrap();
        calls::record(db, &call(86_400.0 + 6.0, "claude-haiku-4-5", None, 0.25)).unwrap();
        let all = calls::since(db, 0.0).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], call(10.0, "claude-haiku-4-5", Some("Triage"), 0.25));
        assert_eq!(calls::since(db, 100.0).unwrap().len(), 2);
        let agents = calls::totals(&all, By::Agent);
        assert_eq!(agents[0].key.as_deref(), Some("Writer"), "costliest first");
        assert!(agents.iter().any(|t| t.key.is_none()), "calls outside any agent");
        let models = calls::totals(&all, By::Model);
        let haiku = models.iter().find(|t| t.key.as_deref() == Some("claude-haiku-4-5")).unwrap();
        assert_eq!((haiku.calls, haiku.input_tokens, haiku.output_tokens, haiku.cost_usd), (2, 200, 20, 0.5));
        let days: Vec<_> = calls::totals(&all, By::Day).into_iter().map(|t| t.key.unwrap()).collect();
        assert_eq!(days, ["1970-01-01", "1970-01-02"]);
        assert_eq!(calls::totals(&all, By::Workflow).len(), 1);
    });
}

#[test]
fn events_and_refusals() {
    on_every_database(|db| {
        events::record(db, 1.0, "request", "POST /tickets", "TaintError", "an untrusted value reaches `html`").unwrap();
        events::record(db, 2.0, "job", "job 4 (fetch)", "IoError", "down").unwrap();
        let all = events::latest(db, false, 10).unwrap();
        assert_eq!(all.iter().map(|e| e.error.as_str()).collect::<Vec<_>>(), ["IoError", "TaintError"]);
        let refusals = events::latest(db, true, 10).unwrap();
        assert_eq!(refusals.len(), 1);
        assert!(refusals[0].is_refusal());
        assert_eq!(refusals[0].subject, "POST /tickets");
    });
}

#[test]
fn eval_runs_over_time() {
    on_every_database(|db| {
        let run = |at: f64, name: &str, score: f64| Run {
            at,
            name: name.into(),
            score,
            threshold: 0.8,
            passed: score >= 0.8,
            rows: 10,
            failed_rows: 1,
            cost_usd: 0.4,
            seconds: 3.5,
            ..Run::default()
        };
        evals::record(db, &run(1.0, "triage", 0.7)).unwrap();
        evals::record(db, &run(2.0, "summary", 0.9)).unwrap();
        evals::record(db, &run(3.0, "triage", 0.85)).unwrap();
        let history = evals::history(db).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!((history[0].passed, history[0].rows, history[0].failed_rows), (false, 10, 1));
        let groups = evals::by_name(history);
        assert_eq!(groups[0].0, "triage");
        assert_eq!(groups[0].1.iter().map(|r| r.score).collect::<Vec<_>>(), [0.7, 0.85]);
    });
}

#[test]
fn journals_are_written_read_and_listed() {
    let dir = std::env::temp_dir().join(format!("grenat-ops-journal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = journal::path(&dir, "onboard", &[json!(1)], &[]);
    assert_eq!(path, journal::path(&dir, "onboard", &[json!(1)], &[]), "a run is named by its arguments");
    assert_ne!(path, journal::path(&dir, "onboard", &[json!(2)], &[]));
    assert_eq!(journal::read(&path).unwrap(), journal::Journal::default());
    journal::append_step(&path, "draft", 0, &json!("A draft")).unwrap();
    journal::append_step(&path, "draft", 1, &json!("Another")).unwrap();
    // a line cut by a crash
    std::fs::OpenOptions::new().append(true).open(&path).and_then(|mut f| std::io::Write::write_all(&mut f, b"{\"step\": \"rev")).unwrap();
    let read = journal::read(&path).unwrap();
    assert_eq!(read.steps.len(), 2);
    assert_eq!((read.steps[1].name.as_str(), read.steps[1].n), ("draft", 1));
    assert_eq!(read.result, None);
    std::fs::write(dir.join("notes.txt"), "not a journal").unwrap();
    std::fs::write(dir.join("x-nothex.jsonl"), "").unwrap();
    let listed = journal::list(&dir);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].workflow, "onboard");
    let found = journal::find(&dir, &listed[0].run).unwrap();
    std::fs::write(&found.path, "").unwrap();
    journal::append_result(&found.path, &json!(42)).unwrap();
    assert_eq!(journal::read(&found.path).unwrap().result, Some(json!(42)));
    assert!(journal::find(&dir, "../etc/passwd").is_none());
    assert!(journal::list(&dir.join("missing")).is_empty());
}
