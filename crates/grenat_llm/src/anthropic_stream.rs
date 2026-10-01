//! The Messages API streamed (`"stream": true`): its events put back
//! together into the message a call without streaming returns.
//!
//! `message_start` carries the message (no content yet) and the input's
//! usage; each content block opens (`content_block_start`), grows by
//! deltas — `text_delta`, `input_json_delta` (a tool call's input, partial
//! JSON), `thinking_delta`, `signature_delta`, `citations_delta` — and
//! closes; `message_delta` gives the stop reason and the usage so far
//! (cumulative); `message_stop` ends it. `ping`s and unknown events are
//! ignored; an `error` event fails the call.

use serde_json::{Map, Value as Json, json};

use crate::sse::Event;
use crate::streaming::{Decoder, Delta, Failed, Out, json};
use crate::*;

#[derive(Default)]
pub(crate) struct MessagesDecoder {
    message: Json,
    blocks: Vec<Json>,
    /// The partial JSON of each tool call's input, by block.
    inputs: Vec<String>,
}

impl Decoder for MessagesDecoder {
    fn event(&mut self, event: &Event, out: &mut Out) -> Result<Option<Response>, Failed> {
        if !matches!(
            event.name.as_str(),
            "message_start" | "content_block_start" | "content_block_delta" | "content_block_stop" | "message_delta"
        ) {
            return match event.name.as_str() {
                "message_stop" => self.finish().map(Some),
                "error" => Err(stream_error(&json(event)?)),
                _ => Ok(None),
            };
        }
        let data = json(event)?;
        let index = data["index"].as_u64().unwrap_or(0) as usize;
        match event.name.as_str() {
            "message_start" => self.message = data["message"].clone(),
            "content_block_start" => {
                while self.blocks.len() <= index {
                    self.blocks.push(Json::Null);
                    self.inputs.push(String::new());
                }
                self.blocks[index] = data["content_block"].clone();
            }
            "content_block_delta" => {
                let Some(block) = self.blocks.get_mut(index) else {
                    return Err(Failed::fatal(format!("a delta for the unopened block {index}")));
                };
                let delta = &data["delta"];
                let piece = |field: &str| delta[field].as_str().unwrap_or_default();
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        append(block, "text", piece("text"));
                        out.send(Delta::Text(piece("text")))?;
                    }
                    Some("input_json_delta") => {
                        self.inputs[index].push_str(piece("partial_json"));
                        let (id, name) = (block["id"].as_str().unwrap_or_default(), block["name"].as_str());
                        let json = piece("partial_json");
                        out.send(Delta::ToolInput { id, name: name.unwrap_or_default(), json })?;
                    }
                    Some("thinking_delta") => append(block, "thinking", piece("thinking")),
                    Some("signature_delta") => append(block, "signature", piece("signature")),
                    Some("citations_delta") => {
                        if !block["citations"].is_array() {
                            block["citations"] = json!([]);
                        }
                        if let Some(list) = block["citations"].as_array_mut() {
                            list.push(delta["citation"].clone());
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(block) = self.blocks.get_mut(index)
                    && block.get("input").is_some()
                {
                    let partial = std::mem::take(&mut self.inputs[index]);
                    // an input that is not JSON reaches the tool empty, and fails its validation
                    block["input"] = if partial.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&partial).unwrap_or_else(|_| Json::Object(Map::new()))
                    };
                }
            }
            _ => {
                // message_delta: the stop reason, the usage so far
                if let Some(changes) = data["delta"].as_object() {
                    for (key, value) in changes {
                        self.message[key] = value.clone();
                    }
                }
                if let Some(usage) = data["usage"].as_object() {
                    if !self.message["usage"].is_object() {
                        self.message["usage"] = json!({});
                    }
                    for (key, value) in usage.iter().filter(|(_, v)| !v.is_null()) {
                        self.message["usage"][key] = value.clone();
                    }
                }
            }
        }
        Ok(None)
    }

    /// The usage `message_start` and the last `message_delta` said.
    fn billed(&self) -> Option<Response> {
        if !self.message["usage"].is_object() {
            return None;
        }
        let mut message = self.message.clone();
        message["content"] = json!([]);
        parse_response(&message).ok()
    }
}

impl MessagesDecoder {
    /// The message, as the API would have answered it without streaming.
    fn finish(&mut self) -> Result<Response, Failed> {
        let mut blocks: Vec<Json> = std::mem::take(&mut self.blocks).into_iter().filter(|b| !b.is_null()).collect();
        // after a decline mid-answer, the fallback model goes on from the
        // text: the blocks before the handoff that cannot be sent back go
        if let Some(handoff) = blocks.iter().rposition(|b| b["type"] == "fallback") {
            if let Some(model) = blocks[handoff]["to"]["model"].as_str() {
                self.message["model"] = json!(model);
            }
            let mut position = 0;
            blocks.retain(|b| {
                position += 1;
                position > handoff || !matches!(b["type"].as_str(), Some("tool_use" | "thinking" | "redacted_thinking"))
            });
        }
        self.message["content"] = Json::Array(blocks);
        parse_response(&self.message).map_err(|e| Failed::fatal(e.message))
    }
}

fn append(block: &mut Json, field: &str, piece: &str) {
    let text = format!("{}{piece}", block[field].as_str().unwrap_or_default());
    block[field] = json!(text);
}

/// An `error` event: `{"type": "error", "error": {"type": …, "message": …}}`.
fn stream_error(data: &Json) -> Failed {
    let kind = data["error"]["type"].as_str().unwrap_or("error");
    let message = data["error"]["message"].as_str().unwrap_or("unknown error");
    let failed = Failed::fatal(format!("{message} ({kind}, while streaming)"));
    match kind {
        "overloaded_error" | "api_error" | "rate_limit_error" | "timeout_error" => Failed { retry: true, ..failed },
        _ => failed,
    }
}
