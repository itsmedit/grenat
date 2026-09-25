//! The overview: what needs attention, what it costs, how it does.

use grenat_ops::jobs::{self, Status};
use grenat_ops::{approvals, calls, evals, events};

use super::{Ctx, Result, page};
use crate::html::{badge, escape, money};

pub(crate) fn show(ctx: &mut Ctx) -> Result {
    let pending = approvals::pending(ctx.db)?.len();
    let counts = jobs::counts(ctx.db)?;
    let count = |s: Status| counts.iter().find(|(c, _)| *c == s).map_or(0, |(_, n)| *n);
    let day = calls::since(ctx.db, ctx.now - 86_400.0)?.iter().map(|c| c.cost_usd).sum::<f64>();
    let week = calls::since(ctx.db, ctx.now - 7.0 * 86_400.0)?.iter().map(|c| c.cost_usd).sum::<f64>();
    let refusals = events::latest(ctx.db, true, 1000)?.iter().filter(|e| e.at >= ctx.now - 7.0 * 86_400.0).count();
    let card = |href: &str, value: String, label: &str| format!("<a class=\"card\" href=\"{href}\"><b>{value}</b><span>{label}</span></a>");
    let cards = [
        card("/approvals", pending.to_string(), "approvals waiting"),
        card("/jobs?status=failed", count(Status::Failed).to_string(), "failed jobs"),
        card("/jobs?status=queued", (count(Status::Queued) + count(Status::Running)).to_string(), "jobs queued or running"),
        card("/jobs?status=waiting", count(Status::Waiting).to_string(), "jobs waiting for a human"),
        card("/costs?days=1", money(day), "spent in 24 h"),
        card("/costs?days=7", money(week), "spent in 7 days"),
        card("/events?only=refusals", refusals.to_string(), "refusals in 7 days"),
    ];
    let evals: String = evals::by_name(evals::history(ctx.db)?)
        .iter()
        .map(|(name, runs)| {
            let last = runs.last().expect("a group has runs");
            let verdict = if last.passed { badge("passed", "ok") } else { badge("failed", "bad") };
            format!("<li>{} {:.0}% {verdict}</li>", escape(name), last.score * 100.0)
        })
        .collect();
    let app = ctx.app;
    let list = |items: Vec<String>| {
        if items.is_empty() { "<span class=\"muted\">none</span>".to_string() } else { escape(&items.join(", ")) }
    };
    let agents = app.agents.iter().map(|a| format!("{} ({})", a.name, a.handlers.join(", "))).collect();
    let body = format!(
        "<div class=\"cards\">{}</div><h2>Evals</h2>{}<h2>The program</h2><dl><dt>Agents</dt><dd>{}</dd>\
         <dt>Workflows</dt><dd>{}</dd><dt>Tools</dt><dd>{}</dd><dt>Routes</dt><dd>{}</dd><dt>MCP servers</dt><dd>{}</dd></dl>",
        cards.concat(),
        if evals.is_empty() { "<p class=\"empty\">No eval has run.</p>".into() } else { format!("<ul>{evals}</ul>") },
        list(agents),
        list(app.workflows.clone()),
        list(app.tools.clone()),
        list(app.routes.clone()),
        list(app.mcp_servers.iter().map(|s| s.name.clone()).collect()),
    );
    page("Overview", "/", body)
}
