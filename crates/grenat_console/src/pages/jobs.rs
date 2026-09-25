//! Jobs: the queue, a job's arguments, error, journal, approvals and cost —
//! and a failed job retried.

use grenat_ops::jobs::{self, Job, Status};
use grenat_ops::{approvals, calls, journal};

use super::{Ctx, Outcome, Result, id, page};
use crate::html::{ago, badge, button, clip, escape, json, money, table};

pub(crate) fn list(ctx: &mut Ctx) -> Result {
    let status = ctx.param("status").and_then(Status::parse);
    let counts = jobs::counts(ctx.db)?;
    let tab = |label: &str, href: String, count: i64, current: bool| {
        let current = if current { " aria-current=\"page\"" } else { "" };
        format!("<a href=\"{href}\"{current}>{label} <span class=\"muted\">{count}</span></a>")
    };
    let mut tabs = vec![tab("All", "/jobs".into(), counts.iter().map(|(_, n)| n).sum(), status.is_none())];
    tabs.extend(counts.iter().map(|(s, n)| tab(s.as_str(), format!("/jobs?status={}", s.as_str()), *n, status == Some(*s))));
    let rows: Vec<Vec<String>> = jobs::list(ctx.db, status, 200)?
        .iter()
        .map(|job| {
            vec![
                format!("<a href=\"/jobs/{0}\">{0}</a>", job.id),
                escape(&job.name),
                clip(&job.args, 40),
                status_badge(job.status),
                format!("<span class=\"num\">{}</span>", job.attempts),
                ago(job.updated_at, ctx.now),
                job.error.as_deref().map(|e| clip(e, 70)).unwrap_or_default(),
            ]
        })
        .collect();
    let body = format!(
        "<div class=\"tabs\">{}</div>{}",
        tabs.concat(),
        table(&["#", "Function", "Arguments", "Status", "Runs", "Updated", "Last error"], &rows, "No job.")
    );
    page("Jobs", "/jobs", body)
}

pub(crate) fn show(ctx: &mut Ctx, id_text: &str) -> Result {
    let Some(job) = id(id_text).map(|id| jobs::get(ctx.db, id)).transpose()?.flatten() else {
        return Ok(Outcome::NotFound);
    };
    let retry = if job.status == Status::Failed {
        format!("<p>{}</p>", button(&format!("/jobs/{}/retry", job.id), "Retry", "primary", ctx.csrf))
    } else {
        String::new()
    };
    let args: serde_json::Value = serde_json::from_str(&job.args).unwrap_or(serde_json::Value::String(job.args.clone()));
    let error = job.error.as_deref().map(|e| format!("<h2>Last error</h2><pre>{}</pre>", escape(e))).unwrap_or_default();
    let spent = calls::of_job(ctx.db, job.id)?;
    let cost = spent.iter().map(|c| c.cost_usd).sum::<f64>();
    let asked: Vec<Vec<String>> = approvals::of_job(ctx.db, job.id)?
        .iter()
        .map(|a| vec![escape(&a.message), super::approvals::decision(a.decision), ago(a.created_at, ctx.now)])
        .collect();
    let body = format!(
        "{notice}{retry}<dl><dt>Function</dt><dd>{name}</dd><dt>Status</dt><dd>{status}</dd><dt>Runs</dt><dd>{attempts}</dd>\
         <dt>Queued</dt><dd>{created}</dd><dt>Updated</dt><dd>{updated}</dd><dt>Next run</dt><dd>{next}</dd>\
         <dt>Model calls</dt><dd>{calls} · {cost}</dd></dl>\
         <h2>Arguments</h2>{args}{error}{journal}<h2>Approvals</h2>{asked}",
        notice = ctx.notice(&[("retried", "Queued again: a workflow resumes from its journal."), ("stale", "Only a failed job can be retried.")]),
        name = escape(&job.name),
        status = status_badge(job.status),
        attempts = job.attempts,
        created = ago(job.created_at, ctx.now),
        updated = ago(job.updated_at, ctx.now),
        next = if job.status == Status::Queued { ago(job.run_at, ctx.now) } else { "—".into() },
        calls = spent.len(),
        cost = money(cost),
        args = json(&args),
        journal = journal_of(ctx, &job, &args),
        asked = table(&["Question", "Decision", "Asked"], &asked, "This job asked nobody."),
    );
    page(format!("Job {}", job.id), "/jobs", body)
}

/// The journal of a job that runs a workflow.
fn journal_of(ctx: &Ctx, job: &Job, args: &serde_json::Value) -> String {
    if !ctx.app.workflows.contains(&job.name) {
        return String::new();
    }
    let args = args.as_array().cloned().unwrap_or_default();
    let path = journal::path(ctx.journal_dir, &job.name, &args, &[]);
    let run = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    match journal::read(&path) {
        Ok(read) if read.steps.is_empty() && read.result.is_none() => "<h2>Journal</h2><p class=\"empty\">No step journaled yet.</p>".into(),
        Ok(read) => format!(
            "<h2>Journal</h2><p><a href=\"/journals/{}\">{} step(s){}</a></p>",
            escape(&run),
            read.steps.len(),
            if read.result.is_some() { ", completed" } else { "" }
        ),
        Err(e) => format!("<h2>Journal</h2><p class=\"bad\">{}</p>", escape(&e)),
    }
}

pub(crate) fn retry(ctx: &mut Ctx, id_text: &str) -> Result {
    let Some(id) = id(id_text) else { return Ok(Outcome::NotFound) };
    let done = if jobs::retry(ctx.db, id, ctx.now)? { "retried" } else { "stale" };
    Ok(Outcome::Redirect(format!("/jobs/{id}?done={done}")))
}

pub(crate) fn status_badge(status: Status) -> String {
    let tone = match status {
        Status::Done => "ok",
        Status::Failed => "bad",
        Status::Waiting => "warn",
        Status::Queued | Status::Running => "muted",
    };
    badge(status.as_str(), tone)
}
