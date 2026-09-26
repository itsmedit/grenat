//! Approvals: the questions jobs wait on, decided here.

use grenat_ops::approvals::{self, Decision};

use super::{Ctx, Outcome, Result, id, page};
use crate::html::{ago, badge, button, clip, escape, table};

pub(crate) fn list(ctx: &mut Ctx) -> Result {
    let pending = approvals::pending(ctx.db)?;
    let decided = approvals::decided(ctx.db, 50)?;
    let rows: Vec<Vec<String>> = pending
        .iter()
        .map(|a| {
            vec![
                escape(&a.message),
                format!("<a href=\"/jobs/{0}\">job {0}</a>", a.job_id),
                ago(a.created_at, ctx.now),
                format!(
                    "{} {}",
                    button(&format!("/approvals/{}/approve", a.id), "Approve", "primary", ctx.csrf),
                    button(&format!("/approvals/{}/deny", a.id), "Deny", "", ctx.csrf)
                ),
            ]
        })
        .collect();
    let history: Vec<Vec<String>> = decided
        .iter()
        .map(|a| {
            vec![
                clip(&a.message, 90),
                format!("<a href=\"/jobs/{0}\">job {0}</a>", a.job_id),
                decision(a.decision),
                ago(a.decided_at.unwrap_or(0.0), ctx.now),
            ]
        })
        .collect();
    let body = format!(
        "{}<p class=\"muted\">A job waits for each of these. Deciding queues it again: it resumes where it stopped.</p>{}<h2>Decided</h2>{}",
        ctx.notice(&[
            ("approved", "Approved: the job runs again."),
            ("denied", "Denied: the job runs again, and fails."),
            ("stale", "This approval was already decided.")
        ]),
        table(&["Question", "Job", "Asked", ""], &rows, "Nothing waits for a human."),
        table(&["Question", "Job", "Decision", "When"], &history, "No decision yet."),
    );
    page("Approvals", "/approvals", body)
}

pub(crate) fn decide(ctx: &mut Ctx, id_text: &str, approve: bool) -> Result {
    let Some(id) = id(id_text) else { return Ok(Outcome::NotFound) };
    let done = if approvals::decide(ctx.db, id, approve, ctx.now)? {
        if approve { "approved" } else { "denied" }
    } else {
        "stale"
    };
    Ok(Outcome::Redirect(format!("/approvals?done={done}")))
}

pub(crate) fn decision(decision: Decision) -> String {
    match decision {
        Decision::Pending => badge("pending", "warn"),
        Decision::Approved => badge("approved", "ok"),
        Decision::Denied => badge("denied", "bad"),
    }
}
