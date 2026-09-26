//! OpenAI's Responses API wire format (`POST /responses`): what OpenAI's own
//! models are best spoken to with — reasoning models take tools there, not
//! through Chat Completions. Requests are stateless (`store: false`): the
//! whole history goes each time, as with the other protocols.

use serde_json::{Map, Value as Json, json};

use crate::catalog::Catalogued;
use crate::*;

/// JSON body of a `POST /responses` request.
pub fn responses_body(request: &Request, provider: &Catalogued) -> Result<Json, LlmError> {
    let model = request.model;
    let mut input = Vec::new();
    for message in &request.messages {
        translate(message, provider, &mut input)?;
    }
    let mut body = json!({"model": model.name, "input": input, "max_output_tokens": model.max_tokens, "store": false});
    if let Some(system) = request.system.as_deref().filter(|s| !s.is_empty()) {
        body["instructions"] = json!(system);
    }
    if let Some(temperature) = model.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(effort) = &model.effort {
        body["reasoning"] = json!({"effort": effort});
    }
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|t| json!({"type": "function", "name": t.name, "description": t.description, "parameters": t.input_schema, "strict": t.strict}))
            .collect();
    }
    if let Some(schema) = &request.output_schema {
        body["text"] = json!({"format": {"type": "json_schema", "name": "answer", "schema": schema, "strict": true}});
    }
    Ok(body)
}

/// One message of the history, as input items.
fn translate(message: &Json, provider: &Catalogued, out: &mut Vec<Json>) -> Result<(), LlmError> {
    let role = message["role"].as_str().unwrap_or("user");
    let blocks = match &message["content"] {
        Json::String(text) => {
            out.push(json!({"role": role, "content": text}));
            return Ok(());
        }
        Json::Array(blocks) => blocks,
        _ => return Ok(()),
    };
    if role == "assistant" {
        let text: String = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
        if !text.is_empty() {
            out.push(json!({"role": "assistant", "content": text}));
        }
        for call in blocks.iter().filter(|b| b["type"] == "tool_use") {
            out.push(json!({"type": "function_call", "call_id": call["id"], "name": call["name"], "arguments": call["input"].to_string()}));
        }
        return Ok(());
    }
    for block in blocks.iter().filter(|b| b["type"] == "tool_result") {
        let mut output = match &block["content"] {
            Json::String(text) => text.clone(),
            Json::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
            other => other.to_string(),
        };
        if block["is_error"] == true {
            output = format!("Error: {output}");
        }
        out.push(json!({"type": "function_call_output", "call_id": block["tool_use_id"], "output": output}));
    }
    let mut parts = Vec::new();
    for block in blocks.iter().filter(|b| b["type"] != "tool_result") {
        parts.push(part(block, provider)?);
    }
    if !parts.is_empty() {
        out.push(json!({"role": role, "content": parts}));
    }
    Ok(())
}

fn part(block: &Json, provider: &Catalogued) -> Result<Json, LlmError> {
    let source = &block["source"];
    let data_url = || {
        format!(
            "data:{};base64,{}",
            source["media_type"].as_str().unwrap_or_default(),
            source["data"].as_str().unwrap_or_default()
        )
    };
    Ok(match (block["type"].as_str(), source["type"].as_str()) {
        (Some("text"), _) => json!({"type": "input_text", "text": block["text"]}),
        (Some("image"), Some("url")) => json!({"type": "input_image", "image_url": source["url"]}),
        (Some("image"), _) => json!({"type": "input_image", "image_url": data_url()}),
        (Some("document"), Some("url")) => json!({"type": "input_file", "file_url": source["url"]}),
        (Some("document"), _) => json!({"type": "input_file", "filename": "document.pdf", "file_data": data_url()}),
        (other, _) => {
            return Err(LlmError::new(format!("unsupported content {other:?} for the provider `{}`", provider.name)));
        }
    })
}

/// A Responses API answer, as content blocks.
pub(crate) fn parse_responses(body: &Json) -> Result<Response, LlmError> {
    let output = body["output"].as_array().ok_or_else(|| LlmError::new("response without `output`"))?;
    let mut content = Vec::new();
    let mut refused = false;
    for item in output {
        match item["type"].as_str() {
            Some("message") => {
                for part in item["content"].as_array().into_iter().flatten() {
                    match part["type"].as_str() {
                        Some("output_text") => content.push(json!({"type": "text", "text": part["text"]})),
                        Some("refusal") => refused = true,
                        _ => {}
                    }
                }
            }
            Some("function_call") => {
                let arguments = item["arguments"].as_str().unwrap_or("{}");
                let input = serde_json::from_str::<Json>(arguments).unwrap_or_else(|_| Json::Object(Map::new()));
                content.push(json!({"type": "tool_use", "id": item["call_id"], "name": item["name"], "input": input}));
            }
            // reasoning and other items stay with the provider
            _ => {}
        }
    }
    let truncated = body["status"] == "incomplete" && body["incomplete_details"]["reason"] == "max_output_tokens";
    let stop_reason = if refused {
        "refusal"
    } else if truncated {
        "max_tokens"
    } else if content.iter().any(|b| b["type"] == "tool_use") {
        "tool_use"
    } else {
        "end_turn"
    };
    let usage = &body["usage"];
    let tokens = |v: &Json| v.as_u64().unwrap_or(0);
    let cached = tokens(&usage["input_tokens_details"]["cached_tokens"]);
    Ok(Response {
        content,
        stop_reason: stop_reason.into(),
        usage: Usage {
            input_tokens: tokens(&usage["input_tokens"]).saturating_sub(cached),
            output_tokens: tokens(&usage["output_tokens"]),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: cached,
        },
        model: body["model"].as_str().unwrap_or_default().to_string(),
    })
}
