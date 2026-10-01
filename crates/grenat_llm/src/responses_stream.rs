//! OpenAI's Responses API streamed (`"stream": true`): typed events, the
//! whole response in the last one.
//!
//! `response.output_text.delta` carries text; a function call opens with
//! `response.output_item.added` (its `call_id` and `name`, at an
//! `output_index`) and its arguments come in
//! `response.function_call_arguments.delta`s. `response.completed` (or
//! `response.incomplete`, cut short) carries the response, read as one
//! without streaming; `response.failed` and `error` fail the call. Other
//! events (reasoning, refusal deltas…) are ignored: the last response
//! holds what they built.

use std::collections::HashMap;

use serde_json::Value as Json;

use crate::responses_wire::parse_responses;
use crate::sse::Event;
use crate::streaming::{Decoder, Delta, Failed, Out, json};
use crate::*;

#[derive(Default)]
pub(crate) struct ResponsesDecoder {
    /// Function calls being written: (call id, name), by output index.
    calls: HashMap<u64, (String, String)>,
    /// Output items finished, by output index: the output, should the
    /// last response not repeat it.
    done: Vec<(u64, Json)>,
}

impl Decoder for ResponsesDecoder {
    fn event(&mut self, event: &Event, out: &mut Out) -> Result<Option<Response>, Failed> {
        let data = json(event)?;
        // the type is in the data; the event's name repeats it
        let kind = data["type"].as_str().unwrap_or(&event.name);
        let index = data["output_index"].as_u64().unwrap_or(0);
        match kind {
            "response.output_text.delta" => out.send(Delta::Text(data["delta"].as_str().unwrap_or_default()))?,
            "response.output_item.added" if data["item"]["type"] == "function_call" => {
                let item = &data["item"];
                let field = |name: &str| item[name].as_str().unwrap_or_default().to_string();
                self.calls.insert(index, (field("call_id"), field("name")));
            }
            "response.function_call_arguments.delta" => {
                let (id, name) = self.calls.get(&index).map(|(i, n)| (i.as_str(), n.as_str())).unwrap_or_default();
                out.send(Delta::ToolInput { id, name, json: data["delta"].as_str().unwrap_or_default() })?;
            }
            "response.output_item.done" => self.done.push((index, data["item"].clone())),
            "response.completed" | "response.incomplete" => {
                let mut response = data["response"].clone();
                let empty = response["output"].as_array().is_none_or(Vec::is_empty);
                if empty && !self.done.is_empty() {
                    self.done.sort_by_key(|(i, _)| *i);
                    response["output"] = self.done.drain(..).map(|(_, item)| item).collect();
                }
                return parse_responses(&response).map(Some).map_err(|e| Failed::fatal(e.message));
            }
            "response.failed" => {
                let error = &data["response"]["error"];
                let message = error["message"].as_str().unwrap_or("the response failed");
                let retry = matches!(error["code"].as_str(), Some("server_error" | "rate_limit_exceeded"));
                return Err(Failed { retry, ..Failed::fatal(format!("{message} (while streaming)")) });
            }
            "error" => {
                let message = data["message"].as_str().unwrap_or("unknown error");
                let retry = matches!(data["code"].as_str(), Some("server_error" | "rate_limit_exceeded"));
                return Err(Failed { retry, ..Failed::fatal(format!("{message} (while streaming)")) });
            }
            _ => {}
        }
        Ok(None)
    }
}
