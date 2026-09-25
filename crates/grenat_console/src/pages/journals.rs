//! Journals: each run of a workflow, its steps and their values.

use grenat_ops::journal;

use super::{Ctx, Outcome, Result, page};
use crate::html::{ago, badge, escape, json, table};

pub(crate) fn list(ctx: &mut Ctx) -> Result {
    let rows: Vec<Vec<String>> = journal::list(ctx.journal_dir)
        .iter()
        .map(|entry| {
            let read = journal::read(&entry.path).unwrap_or_default();
            vec![
                escape(&entry.workflow),
                format!("<a href=\"/journals/{0}\">{0}</a>", escape(&entry.run)),
                format!("<span class=\"num\">{}</span>", read.steps.len()),
                if read.result.is_some() { badge("completed", "ok") } else { badge("in progress", "warn") },
                ago(entry.modified, ctx.now),
            ]
        })
        .collect();
    let body = format!(
        "<p class=\"muted\">Each run of a workflow, named by its arguments, in <code>{}</code>. A run that stopped resumes from its last journaled step.</p>{}",
        escape(&ctx.journal_dir.display().to_string()),
        table(&["Workflow", "Run", "Steps", "State", "Written"], &rows, "No workflow has run here.")
    );
    page("Journals", "/journals", body)
}

pub(crate) fn show(ctx: &mut Ctx, run: &str) -> Result {
    // only a journal of the listing: a name never becomes a path
    let Some(entry) = journal::find(ctx.journal_dir, run) else { return Ok(Outcome::NotFound) };
    let read = journal::read(&entry.path)?;
    let steps: Vec<Vec<String>> = read
        .steps
        .iter()
        .map(|step| {
            let name = if step.n == 0 { format!(":{}", step.name) } else { format!(":{} #{}", step.name, step.n + 1) };
            vec![escape(&name), json(&step.value)]
        })
        .collect();
    let result = match &read.result {
        Some(value) => format!("<h2>Result</h2>{}", json(value)),
        None => "<h2>Result</h2><p class=\"empty\">Not completed: the next run resumes after the last step.</p>".into(),
    };
    let body = format!(
        "<dl><dt>Workflow</dt><dd>{}</dd><dt>Written</dt><dd>{}</dd></dl><h2>Steps</h2>{}{result}",
        escape(&entry.workflow),
        ago(entry.modified, ctx.now),
        table(&["Step", "Value"], &steps, "No step journaled.")
    );
    page(format!("Run {}", entry.run), "/journals", body)
}
