//! Evals: the score of each eval, run after run.

use grenat_ops::evals::{self, Run};

use super::{Ctx, Result, page};
use crate::html::{ago, badge, escape, money, table};

pub(crate) fn show(ctx: &mut Ctx) -> Result {
    let groups = evals::by_name(evals::history(ctx.db)?);
    let summary: Vec<Vec<String>> = groups
        .iter()
        .map(|(name, runs)| {
            let last = runs.last().expect("a group has runs");
            vec![
                escape(name),
                format!("{:.0}%", last.score * 100.0),
                format!("{:.0}%", last.threshold * 100.0),
                if last.passed { badge("passed", "ok") } else { badge("failed", "bad") },
                sparkline(runs),
                format!("<span class=\"num\">{}</span>", runs.len()),
                ago(last.at, ctx.now),
            ]
        })
        .collect();
    let latest: Vec<Vec<String>> = groups
        .iter()
        .flat_map(|(_, runs)| runs.iter())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(30)
        .map(|run| {
            vec![
                escape(&run.name),
                format!("{:.0}%", run.score * 100.0),
                format!("<span class=\"num\">{} ({} failed)</span>", run.rows, run.failed_rows),
                money(run.cost_usd),
                format!("{:.1} s", run.seconds),
                ago(run.at, ctx.now),
            ]
        })
        .collect();
    let body = format!(
        "<p class=\"muted\">Each <code>grenat eval</code> is kept: quality over time, next to what it cost.</p>{}<h2>Latest runs</h2>{}",
        table(&["Eval", "Score", "Threshold", "Last run", "Over time", "Runs", "When"], &summary, "No eval has run: grenat eval <file>."),
        table(&["Eval", "Score", "Rows", "Cost", "Time", "When"], &latest, "No run."),
    );
    page("Evals", "/evals", body)
}

/// The scores of `runs` (0 to 1), and the threshold dashed.
fn sparkline(runs: &[Run]) -> String {
    const W: f64 = 120.0;
    const H: f64 = 28.0;
    let y = |score: f64| H - score.clamp(0.0, 1.0) * (H - 4.0) - 2.0;
    let step = if runs.len() > 1 { W / (runs.len() - 1) as f64 } else { 0.0 };
    let points: Vec<String> = runs.iter().enumerate().map(|(i, r)| format!("{:.1},{:.1}", i as f64 * step, y(r.score))).collect();
    let threshold = y(runs.last().map_or(0.0, |r| r.threshold));
    format!(
        "<svg width=\"{W}\" height=\"{H}\" viewBox=\"0 0 {W} {H}\" role=\"img\" aria-label=\"scores over time\">\
         <line x1=\"0\" x2=\"{W}\" y1=\"{threshold:.1}\" y2=\"{threshold:.1}\" stroke=\"currentColor\" stroke-dasharray=\"3 3\" opacity=\".4\"/>\
         <polyline points=\"{}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\"/></svg>",
        points.join(" ")
    )
}
