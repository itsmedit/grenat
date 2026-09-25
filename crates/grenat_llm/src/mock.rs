//! Mocked model: replies given as values, shaped into whatever each request
//! expects (plain text, structured output, or the agent's `final_answer`
//! tool call), so that tests say *what* the model answers, not *how*.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value as Json, json};

use crate::*;

/// Name of the tool through which agents give their final answer.
const FINAL_TOOL: &str = "final_answer";

/// One reply of a mocked model.
#[derive(Debug, Clone, PartialEq)]
pub enum MockReply {
    /// The answer: text, or the JSON of the expected structure.
    Answer(Json),
    /// A call to one of the agent's tools.
    Tool { name: String, input: Json },
    /// The call fails.
    Error(String),
}

/// Replays its replies in order, whatever the request.
pub struct Mock {
    label: String,
    replies: Mutex<VecDeque<MockReply>>,
}

impl Mock {
    /// `label` names the mock in errors (the model it stands for).
    pub fn new(label: impl Into<String>, replies: impl IntoIterator<Item = MockReply>) -> Mock {
        Mock { label: label.into(), replies: Mutex::new(replies.into_iter().collect()) }
    }

    /// Replies not consumed yet.
    pub fn remaining(&self) -> usize {
        self.replies.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

impl Provider for Mock {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        let reply = self
            .replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .ok_or_else(|| LlmError::new(format!("the mock of {} has no reply left", self.label)))?;
        let response = match reply {
            MockReply::Error(message) => return Err(LlmError::new(message)),
            MockReply::Tool { name, input } => Response::tool_call("mock_tool", &name, input),
            MockReply::Answer(answer) => shape(answer, request),
        };
        Ok(Response { model: "mock".into(), ..response.with_usage(0, 0) })
    }
}

/// The answer as the request expects it.
fn shape(answer: Json, request: &Request) -> Response {
    if let Some(tool) = request.tools.iter().find(|t| t.name == FINAL_TOOL) {
        return Response::tool_call("mock_final", FINAL_TOOL, wrap(answer, &tool.input_schema));
    }
    match (&request.output_schema, answer) {
        (Some(schema), answer) => Response::json_reply(wrap(answer, schema)),
        (None, Json::String(text)) => Response::text_reply(text),
        (None, other) => Response::text_reply(other.to_string()),
    }
}

/// `{"value": answer}` when the schema wraps a non-object type that way.
fn wrap(answer: Json, schema: &Json) -> Json {
    let wraps = schema["properties"].as_object().is_some_and(|p| p.len() == 1 && p.contains_key("value"));
    let already = answer.as_object().is_some_and(|o| o.contains_key("value"));
    if wraps && !already { json!({ "value": answer }) } else { answer }
}
