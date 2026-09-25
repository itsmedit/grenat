//! Grenat's generators: `grenat new --app` lays out an application,
//! `grenat generate` adds its parts — agents, workflows, records (with their
//! migrations), tools, evals — each with its tests, and requires them from
//! `src/app.grn`. Code only, never hidden configuration: what is generated is
//! read, changed and owned like the rest.

mod app;
mod fields;
mod generators;
mod names;
mod templates;
mod write;

pub use app::{APP_FILE, create_app};
pub use generators::{Kind, generate};
pub use write::Change;
