//! Messages API wire format: request bodies (their prompt-cache breakpoints
//! placed by [`crate::caching`]) and response parsing.

use serde_json::{Value as Json, json};

use crate::*;

/// JSON body of a `POST /v1/messages` request.
pub fn request_body(request: &Request) -> Json {
    let model = request.model;
    let mut body = json!({
        "model": model.name,
        "max_tokens": model.max_tokens,
        "messages": request.messages,
    });
    if let Some(system) = &request.system {
        body["system"] = json!(system);
    }
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema, "strict": t.strict}))
            .collect();
    }
    if model.fallbacks {
        body["fallbacks"] = json!("default");
    }
    if let Some(temperature) = model.temperature {
        body["temperature"] = json!(temperature);
    }
    let mut output_config = serde_json::Map::new();
    if let Some(schema) = &request.output_schema {
        output_config.insert("format".into(), json!({"type": "json_schema", "schema": schema}));
    }
    if let Some(effort) = &model.effort {
        output_config.insert("effort".into(), json!(effort));
    }
    if !output_config.is_empty() {
        body["output_config"] = Json::Object(output_config);
    }
    crate::caching::mark(&mut body, request);
    body
}

pub(crate) fn parse_response(body: &Json) -> Result<Response, LlmError> {
    let content = body["content"].as_array().cloned().ok_or_else(|| LlmError::new("response without `content`"))?;
    Ok(Response {
        content,
        stop_reason: body["stop_reason"].as_str().unwrap_or_default().to_string(),
        usage: usage_of(&body["usage"]),
        model: body["model"].as_str().unwrap_or_default().to_string(),
        declined: declined(&body["usage"]),
    })
}

/// The tokens of a `usage` object (the call's, or an attempt's).
fn usage_of(usage: &Json) -> Usage {
    let tokens = |field: &str| usage[field].as_u64().unwrap_or(0);
    Usage {
        input_tokens: tokens("input_tokens"),
        output_tokens: tokens("output_tokens"),
        cache_creation_input_tokens: tokens("cache_creation_input_tokens"),
        cache_read_input_tokens: tokens("cache_read_input_tokens"),
        cache_creation_1h_input_tokens: usage["cache_creation"]["ephemeral_1h_input_tokens"].as_u64().unwrap_or(0),
    }
}

/// The attempts billed besides the answer, from `usage.iterations`: after
/// a server-side fallback (a `fallback_message` entry is the model that
/// answered, the top-level usage), each `message` entry is a model that
/// declined. One that declined after writing is billed — input and output
/// —; one that declined before writing is billed only in some refusal
/// categories, which the answer does not name: not counted.
fn declined(usage: &Json) -> Vec<Attempt> {
    let Some(iterations) = usage["iterations"].as_array() else { return Vec::new() };
    if !iterations.iter().any(|i| i["type"] == "fallback_message") {
        return Vec::new();
    }
    iterations
        .iter()
        .filter(|i| i["type"] == "message" && i["output_tokens"].as_u64().unwrap_or(0) > 0)
        .map(|i| Attempt { model: i["model"].as_str().unwrap_or_default().to_string(), usage: usage_of(i) })
        .collect()
}
