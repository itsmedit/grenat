//! Evaluation: one module per responsibility, all extending [`Interp`](crate::Interp).

mod agents;
mod assign;
mod budget;
mod call;
mod capabilities;
mod concurrency;
mod construct;
mod doubles;

pub(crate) use doubles::{HttpStub, ShellStub};
mod errors;
mod expr;
mod human;
pub(crate) mod mcp;
mod native;
mod ops;
mod pattern;
pub(crate) mod workflow;

pub(crate) use assign::set_field;
pub(crate) use errors::{error_is_a, is_error_name};
pub(crate) use ops::compare;
