//! The console's pages and actions, one module per section.

mod approvals;
mod costs;
mod evals;
mod events;
mod jobs;
mod journals;
mod mcp;
mod overview;

use std::path::Path;

use grenat_db::Connection;

use crate::{Application, Mcp, form};

/// What a page is made from.
pub(crate) struct Ctx<'a> {
    pub db: &'a mut dyn Connection,
    pub mcp: &'a mut dyn Mcp,
    pub app: &'a Application,
    pub journal_dir: &'a Path,
    pub now: f64,
    pub query: Vec<(String, String)>,
    /// The secret forms carry.
    pub csrf: &'a str,
}

impl Ctx<'_> {
    pub fn param(&self, name: &str) -> Option<&str> {
        form::get(&self.query, name)
    }

    /// The notice for `?done=…` after an action, if any.
    pub fn notice(&self, texts: &[(&str, &str)]) -> String {
        let done = self.param("done").unwrap_or_default();
        texts.iter().find(|(key, _)| *key == done).map(|(_, text)| crate::html::notice(text)).unwrap_or_default()
    }
}

pub(crate) enum Outcome {
    Page {
        title: String,
        section: &'static str,
        body: String,
    },
    /// After an action: the page to show.
    Redirect(String),
    NotFound,
}

pub(crate) type Result = std::result::Result<Outcome, String>;

pub(crate) fn page(title: impl Into<String>, section: &'static str, body: String) -> Result {
    Ok(Outcome::Page { title: title.into(), section, body })
}

pub(crate) fn route(ctx: &mut Ctx, method: &str, segments: &[&str]) -> Result {
    match (method, segments) {
        ("GET", []) => overview::show(ctx),
        ("GET", ["approvals"]) => approvals::list(ctx),
        ("POST", ["approvals", id, decision @ ("approve" | "deny")]) => {
            approvals::decide(ctx, id, *decision == "approve")
        }
        ("GET", ["jobs"]) => jobs::list(ctx),
        ("GET", ["jobs", id]) => jobs::show(ctx, id),
        ("POST", ["jobs", id, "retry"]) => jobs::retry(ctx, id),
        ("GET", ["journals"]) => journals::list(ctx),
        ("GET", ["journals", run]) => journals::show(ctx, run),
        ("GET", ["costs"]) => costs::show(ctx),
        ("GET", ["evals"]) => evals::show(ctx),
        ("GET", ["events"]) => events::list(ctx),
        ("GET", ["mcp"]) => mcp::list(ctx),
        ("GET", ["mcp", server]) => mcp::tools(ctx, server),
        _ => Ok(Outcome::NotFound),
    }
}

/// An id in a path.
pub(crate) fn id(text: &str) -> Option<i64> {
    text.parse().ok().filter(|n| *n > 0)
}
