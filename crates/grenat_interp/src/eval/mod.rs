//! Evaluation: one module per responsibility, all extending [`Interp`](crate::Interp).

mod agents;
pub(crate) mod approvals;
mod assign;
pub(crate) mod batch;
mod budget;
mod call;
mod capabilities;
mod concurrency;
mod construct;
pub(crate) mod conversation;
mod doubles;

pub(crate) use doubles::{HttpStub, ShellStub};
mod errors;
pub(crate) mod exposed;
mod expr;
mod human;
mod jobs;
pub(crate) mod mcp;
mod native;
mod ops;
mod pattern;
pub(crate) mod store;
pub(crate) mod records;
pub(crate) mod workflow;

pub(crate) use assign::set_field;
pub(crate) use errors::{error_is_a, is_error_name};
pub(crate) use ops::compare;
