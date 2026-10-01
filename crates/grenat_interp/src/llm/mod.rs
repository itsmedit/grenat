//! Bridge between the language and LLMs, one module per responsibility.

mod agent_loop;
mod answer_text;
mod json;
mod judge;
mod model;
mod prompt;
mod schema;
mod stream;

pub(crate) use agent_loop::tool_output;
pub(crate) use json::*;
pub(crate) use prompt::text_to;
pub(crate) use schema::*;
