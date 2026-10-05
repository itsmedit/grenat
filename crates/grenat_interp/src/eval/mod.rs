//! Evaluation: one module per responsibility, all extending [`Interp`](crate::Interp).

mod agents;
pub(crate) mod approvals;
mod assign;
pub(crate) mod batch;
mod binding;
mod budget;
mod call;
mod capabilities;
mod clock;
mod concurrency;
mod construct;
pub(crate) mod conversation;
mod doubles;
pub(crate) mod embeddings;
mod environment;

pub(crate) use doubles::{HttpStub, ShellStub};
mod errors;
pub(crate) mod exposed;
mod expr;
mod foreign;
mod human;
mod jobs;
mod ledger;
mod mail_double;
pub(crate) mod mcp;
mod native;
mod ops;
mod pattern;
pub(crate) mod records;
mod requests;
pub(crate) mod secrets;
pub(crate) mod store;
mod testing;
pub(crate) mod transcription;
pub(crate) mod vectors;
pub(crate) mod workflow;

pub(crate) use assign::set_field;
pub(crate) use errors::{error_is_a, is_error_name};
pub(crate) use foreign::bridge_log;
pub(crate) use ops::compare;
