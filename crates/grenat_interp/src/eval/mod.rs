//! Evaluation: one module per responsibility, all extending [`Interp`](crate::Interp).

mod agents;
mod assign;
mod budget;
mod call;
mod capabilities;
mod concurrency;
mod construct;
mod errors;
mod expr;
mod human;
mod native;
mod ops;
mod pattern;

pub(crate) use assign::set_field;
pub(crate) use errors::{error_is_a, is_error_name};
pub(crate) use ops::compare;
