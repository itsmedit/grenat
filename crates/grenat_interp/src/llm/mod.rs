//! Pont entre le langage et les LLM, un module par responsabilité.

mod agent_loop;
mod json;
mod model;
mod prompt;
mod schema;

pub(crate) use json::*;
pub(crate) use schema::*;
