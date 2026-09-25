//! Events: what failed while serving, and the refusals among them.

use grenat_ops::events;

use super::{Ctx, Result, page};
use crate::html::{ago, badge, clip, escape, table};

pub(crate) fn list(ctx: &mut Ctx) -> Result {
    let refusals = ctx.param("only") == Some("refusals");
    let rows: Vec<Vec<String>> = events::latest(ctx.db, refusals, 200)?
        .iter()
        .map(|e| {
            let tone = if e.is_refusal() { "warn" } else { "bad" };
            vec![ago(e.at, ctx.now), escape(&e.source), escape(&e.subject), badge(&e.error, tone), clip(&e.message, 120)]
        })
        .collect();
    let tab = |href: &str, label: &str, current: bool| {
        format!("<a href=\"{href}\"{}>{label}</a>", if current { " aria-current=\"page\"" } else { "" })
    };
    let body = format!(
        "<div class=\"tabs\">{}{}</div><p class=\"muted\">A refusal is the program stopping itself: untrusted data at a sink \
         (<code>TaintError</code>), an effect not granted (<code>CapabilityError</code>), a human's no, a budget spent.</p>{}",
        tab("/events", "All", !refusals),
        tab("/events?only=refusals", "Refusals", refusals),
        table(&["When", "Source", "What", "Error", "Message"], &rows, if refusals { "No refusal." } else { "Nothing failed." })
    );
    page("Events", "/events", body)
}
