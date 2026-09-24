//! Bridge between the language and LLMs, one module per responsibility.

mod agent_loop;
mod json;
mod model;
mod prompt;
mod schema;

pub(crate) use json::*;
pub(crate) use schema::*;
