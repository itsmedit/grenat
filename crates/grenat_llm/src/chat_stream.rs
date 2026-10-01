//! Chat Completions streamed (`"stream": true`): chunks, each a `data:`
//! event, put back together into the answer a call without streaming
//! returns, then read as one.
//!
//! A chunk's `choices[0].delta` carries a piece of `content`, of `refusal`,
//! or of `tool_calls` — the call at `index`, its `id` and name in its first
//! piece, its `arguments` in pieces — and `finish_reason` comes last. The
//! usage comes in a last chunk without choices (`stream_options:
//! {"include_usage": true}`), or with the last choice where a provider
//! sends it unasked (Groq in `x_groq.usage`). `data: [DONE]` ends it; a
//! chunk `{"error": …}` fails the call.

use serde_json::{Value as Json, json};

use crate::openai_wire::parse_chat;
use crate::sse::Event;
use crate::streaming::{Decoder, Delta, Failed, Out, json};
use crate::*;

#[derive(Default)]
pub(crate) struct ChatDecoder {
    model: String,
    content: String,
    refusal: String,
    /// (id, name, arguments), by index.
    calls: Vec<(String, String, String)>,
    finish_reason: Option<String>,
    usage: Json,
}

impl Decoder for ChatDecoder {
    fn event(&mut self, event: &Event, out: &mut Out) -> Result<Option<Response>, Failed> {
        if event.data.trim() == "[DONE]" {
            return self.finish().map(Some);
        }
        let chunk = json(event)?;
        if !chunk["error"].is_null() {
            let error = &chunk["error"];
            let message = error["message"].as_str().or_else(|| error.as_str()).unwrap_or("unknown error");
            return Err(Failed::fatal(format!("{message} (while streaming)")));
        }
        if let Some(model) = chunk["model"].as_str().filter(|m| !m.is_empty()) {
            self.model = model.to_string();
        }
        for usage in [&chunk["usage"], &chunk["x_groq"]["usage"]] {
            if usage.is_object() {
                self.usage = usage.clone();
            }
        }
        let choice = &chunk["choices"][0];
        let delta = &choice["delta"];
        if let Some(text) = delta["content"].as_str() {
            self.content.push_str(text);
            out.send(Delta::Text(text))?;
        }
        if let Some(text) = delta["refusal"].as_str() {
            self.refusal.push_str(text);
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let index = call["index"].as_u64().map_or(self.calls.len(), |i| i as usize);
            while self.calls.len() <= index {
                self.calls.push(Default::default());
            }
            let entry = &mut self.calls[index];
            if let Some(id) = call["id"].as_str() {
                entry.0 = id.to_string();
            }
            if let Some(name) = call["function"]["name"].as_str() {
                entry.1.push_str(name);
            }
            let piece = call["function"]["arguments"].as_str().unwrap_or_default();
            entry.2.push_str(piece);
            out.send(Delta::ToolInput { id: &entry.0, name: &entry.1, json: piece })?;
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
        Ok(None)
    }

    /// Some servers close the stream without `[DONE]`: an answer that said
    /// why it finished is whole.
    fn ended(&mut self) -> Result<Response, Failed> {
        if self.finish_reason.is_some() {
            return self.finish();
        }
        Err(Failed::retry("the stream ended before the answer was complete"))
    }
}

impl ChatDecoder {
    fn finish(&mut self) -> Result<Response, Failed> {
        let calls: Vec<Json> = self
            .calls
            .iter()
            .map(|(id, name, arguments)| {
                json!({"id": id, "type": "function", "function": {"name": name, "arguments": arguments}})
            })
            .collect();
        let mut message = json!({"role": "assistant", "content": self.content});
        if !calls.is_empty() {
            message["tool_calls"] = Json::Array(calls);
        }
        if !self.refusal.is_empty() {
            message["refusal"] = json!(self.refusal);
        }
        let body = json!({
            "model": self.model,
            "choices": [{"message": message, "finish_reason": self.finish_reason}],
            "usage": self.usage,
        });
        parse_chat(&body).map_err(|e| Failed::fatal(e.message))
    }
}
