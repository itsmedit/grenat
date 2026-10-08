//! Model calls as the OpenTelemetry GenAI semantic conventions describe
//! them (<https://github.com/open-telemetry/semantic-conventions-genai>):
//! a `CLIENT` span `chat <model>` (or `embeddings <model>`) with
//! `gen_ai.operation.name`, `gen_ai.provider.name`, `gen_ai.request.model`,
//! `gen_ai.response.model`, `gen_ai.response.finish_reasons` and the
//! `gen_ai.usage.*` tokens (cache reads and writes included), plus
//! `grenat.usage.cost_usd`, what the call cost in dollars.
//!
//! By default a prompt is its size only (`grenat.prompt.messages`,
//! `grenat.prompt.bytes`); with the content captured, the opt-in
//! attributes `gen_ai.system_instructions`, `gen_ai.input.messages` and
//! `gen_ai.output.messages` carry the text too (images, documents and
//! audio as their type only).

use grenat_llm::{ModelConfig, Request, Response, Usage};
use grenat_otel::{Kind, SpanStart};
use serde_json::{Value as Json, json};

use crate::prelude::*;

/// The provider as the conventions name it (`gen_ai.provider.name`):
/// their well-known value where there is one, else Grenat's name.
fn provider_name(provider: &str) -> &str {
    match provider {
        "gemini" => "gcp.gemini",
        "mistral" => "mistral_ai",
        "xai" => "x_ai",
        other => other,
    }
}

/// What every model call's span starts with.
fn model_span(interp: &Interp, operation: &str, model: &ModelConfig) -> SpanStart {
    let mut start = SpanStart::new(format!("{operation} {}", model.name), Kind::Client)
        .attr("gen_ai.operation.name", operation)
        .attr("gen_ai.provider.name", provider_name(&model.provider))
        .attr("gen_ai.request.model", model.name.as_str());
    if let Some(frame) = interp.agents.last() {
        start = start.attr("gen_ai.agent.name", frame.agent.ty.to_string());
    }
    start
}

/// `chat <model>`: a prompt, a turn of an agent, of a conversation, a judge.
pub(crate) fn chat_span(interp: &Interp, request: &Request) -> SpanStart {
    let bytes = request.system.as_ref().map_or(0, String::len)
        + request.messages.iter().map(|m| m.to_string().len()).sum::<usize>();
    let mut start = model_span(interp, "chat", request.model)
        .attr("grenat.prompt.messages", request.messages.len())
        .attr("grenat.prompt.bytes", bytes)
        .attr("grenat.request.tools", request.tools.len());
    if interp.tracer.as_ref().is_some_and(|t| t.capture_content()) {
        if let Some(system) = &request.system {
            start = start.attr("gen_ai.system_instructions", json!([{"type": "text", "content": system}]).to_string());
        }
        let messages: Vec<Json> = request.messages.iter().map(message).collect();
        start = start.attr("gen_ai.input.messages", Json::Array(messages).to_string());
    }
    start
}

/// `embeddings <model>`: texts embedded, counted.
pub(crate) fn embedding_span(interp: &Interp, model: &ModelConfig, texts: usize) -> SpanStart {
    model_span(interp, "embeddings", model).attr("grenat.embeddings.inputs", texts)
}

/// `transcription <model>`: audio turned into text (not a value the
/// conventions list: a custom one, as they allow).
pub(crate) fn transcription_span(interp: &Interp, model: &ModelConfig, bytes: usize) -> SpanStart {
    model_span(interp, "transcription", model).attr("grenat.audio.bytes", bytes)
}

impl<'p> Interp<'p> {
    /// An attempt of the call running now, counted on its span: its tokens
    /// and its cost add up over the attempts a fallback billed.
    pub(crate) fn span_usage(&mut self, usage: &Usage, cost_usd: f64) {
        let Some(span) = self.spans.last_mut() else { return };
        if span.get("gen_ai.operation.name").is_none() {
            return;
        }
        let tokens = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        span.add_int("gen_ai.usage.input_tokens", tokens(usage.prompt_tokens()));
        span.add_int("gen_ai.usage.output_tokens", tokens(usage.output_tokens));
        span.add_int("gen_ai.usage.cache_read.input_tokens", tokens(usage.cache_read_input_tokens));
        span.add_int("gen_ai.usage.cache_write.input_tokens", tokens(usage.cache_creation_input_tokens));
        span.add_float("grenat.usage.cost_usd", cost_usd);
    }

    /// What answered the call running now: the model, why it stopped, and
    /// (content captured) what it said.
    pub(crate) fn span_response(&mut self, response: &Response) {
        if !self.tracing() {
            return;
        }
        let capture = self.tracer.as_ref().is_some_and(|t| t.capture_content());
        self.span_set("gen_ai.response.model", response.model.as_str());
        self.span_set("gen_ai.response.finish_reasons", vec![response.stop_reason.clone()]);
        if !response.declined.is_empty() {
            self.span_set("grenat.declined_attempts", response.declined.len());
        }
        if capture {
            let output = message(&json!({"role": "assistant", "content": response.content}));
            self.span_set("gen_ai.output.messages", json!([output]).to_string());
        }
    }
}

/// A message as the conventions write it: its role and its parts.
fn message(message: &Json) -> Json {
    let parts: Vec<Json> = match &message["content"] {
        Json::String(text) => vec![json!({"type": "text", "content": text})],
        Json::Array(blocks) => blocks.iter().map(part).collect(),
        _ => Vec::new(),
    };
    json!({"role": message["role"].as_str().unwrap_or("user"), "parts": parts})
}

/// A content block: text, a tool call and its answer as they are; an
/// image, a document or audio as its type only.
fn part(block: &Json) -> Json {
    match block["type"].as_str().unwrap_or_default() {
        "text" => json!({"type": "text", "content": block["text"]}),
        "tool_use" => {
            json!({"type": "tool_call", "id": block["id"], "name": block["name"], "arguments": block["input"]})
        }
        "tool_result" => {
            json!({"type": "tool_call_response", "id": block["tool_use_id"], "response": block["content"]})
        }
        other => json!({"type": other}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_by_their_conventional_names() {
        assert_eq!(provider_name("anthropic"), "anthropic");
        assert_eq!(provider_name("gemini"), "gcp.gemini");
        assert_eq!(provider_name("mistral"), "mistral_ai");
        assert_eq!(provider_name("ollama"), "ollama");
    }

    #[test]
    fn messages_as_parts_without_binary_content() {
        let user = json!({"role": "user", "content": [
            {"type": "text", "text": "Read this"},
            {"type": "image", "source": {"data": "iVBORw0KGgo="}},
        ]});
        assert_eq!(
            message(&user),
            json!({"role": "user", "parts": [{"type": "text", "content": "Read this"}, {"type": "image"}]})
        );
        let tool = json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "search", "input": {"q": "x"}}]});
        assert_eq!(message(&tool)["parts"][0]["type"], "tool_call");
        assert_eq!(message(&json!({"role": "user", "content": "hi"}))["parts"][0]["content"], "hi");
    }
}
