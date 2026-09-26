//! Chat Completions wire format: Grenat's requests — messages as content
//! blocks (text, images, documents, tool calls and results) — translated for
//! OpenAI and the providers that speak its protocol, and their answers
//! translated back into blocks.

use serde_json::{Map, Value as Json, json};

use crate::catalog::Catalogued;
use crate::*;

/// JSON body of a `POST /chat/completions` request.
pub fn chat_body(request: &Request, provider: &Catalogued) -> Result<Json, LlmError> {
    let model = request.model;
    let mut system = request.system.clone().unwrap_or_default();
    let mut messages = Vec::new();
    for message in &request.messages {
        translate(message, provider, &mut messages)?;
    }
    let mut body = json!({"model": model.name});
    body[provider.max_tokens_field] = json!(model.max_tokens);
    if let Some(temperature) = model.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(effort) = &model.effort {
        body["reasoning_effort"] = json!(effort);
    }
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|t| {
                let mut function = json!({"name": t.name, "description": t.description, "parameters": t.input_schema});
                if t.strict && provider.strict_schemas {
                    function["strict"] = json!(true);
                }
                json!({"type": "function", "function": function})
            })
            .collect();
    }
    if let Some(schema) = &request.output_schema {
        if provider.strict_schemas {
            body["response_format"] =
                json!({"type": "json_schema", "json_schema": {"name": "answer", "schema": schema, "strict": true}});
        } else {
            // JSON mode: the schema goes into the instructions
            body["response_format"] = json!({"type": "json_object"});
            let instruction = format!("Answer with a single JSON object that follows this JSON Schema:\n{schema}");
            system = if system.is_empty() { instruction } else { format!("{system}\n\n{instruction}") };
        }
    }
    if !system.is_empty() {
        messages.insert(0, json!({"role": "system", "content": system}));
    }
    body["messages"] = Json::Array(messages);
    Ok(body)
}

/// One message of the history, as one or several Chat Completions messages.
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
        let calls: Vec<Json> = blocks
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| json!({"id": b["id"], "type": "function", "function": {"name": b["name"], "arguments": b["input"].to_string()}}))
            .collect();
        let mut assistant =
            json!({"role": "assistant", "content": if text.is_empty() { Json::Null } else { json!(text) }});
        if !calls.is_empty() {
            assistant["tool_calls"] = Json::Array(calls);
        }
        out.push(assistant);
        return Ok(());
    }
    // tool results first: they answer the calls of the message before
    for block in blocks.iter().filter(|b| b["type"] == "tool_result") {
        let mut content = match &block["content"] {
            Json::String(text) => text.clone(),
            Json::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
            other => other.to_string(),
        };
        if block["is_error"] == true {
            content = format!("Error: {content}");
        }
        out.push(json!({"role": "tool", "tool_call_id": block["tool_use_id"], "content": content}));
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

/// A content block of a user message.
fn part(block: &Json, provider: &Catalogued) -> Result<Json, LlmError> {
    let source = &block["source"];
    let data_url = || {
        format!(
            "data:{};base64,{}",
            source["media_type"].as_str().unwrap_or_default(),
            source["data"].as_str().unwrap_or_default()
        )
    };
    Ok(match block["type"].as_str() {
        Some("text") => json!({"type": "text", "text": block["text"]}),
        Some("image") if source["type"] == "url" => json!({"type": "image_url", "image_url": {"url": source["url"]}}),
        Some("image") => json!({"type": "image_url", "image_url": {"url": data_url()}}),
        Some("document") if !provider.documents => {
            return Err(LlmError::new(format!("the provider `{}` does not read PDF documents", provider.name)));
        }
        Some("document") if source["type"] == "url" => {
            return Err(LlmError::new(format!(
                "the provider `{}` reads PDF documents given as data (`Pdf.read`), not by URL",
                provider.name
            )));
        }
        Some("document") => json!({"type": "file", "file": {"filename": "document.pdf", "file_data": data_url()}}),
        other => {
            return Err(LlmError::new(format!("unsupported content {other:?} for the provider `{}`", provider.name)));
        }
    })
}

/// A Chat Completions answer, as content blocks.
pub(crate) fn parse_chat(body: &Json) -> Result<Response, LlmError> {
    let choice = &body["choices"][0];
    let message = &choice["message"];
    if message.is_null() {
        return Err(LlmError::new("response without `choices[0].message`"));
    }
    let mut content = Vec::new();
    if let Some(text) = message["content"].as_str().filter(|t| !t.is_empty()) {
        content.push(json!({"type": "text", "text": text}));
    }
    for call in message["tool_calls"].as_array().into_iter().flatten() {
        let arguments = call["function"]["arguments"].as_str().unwrap_or("{}");
        // arguments that are not JSON reach the tool as they are, and fail its validation
        let input = serde_json::from_str::<Json>(arguments).unwrap_or_else(|_| Json::Object(Map::new()));
        content.push(json!({"type": "tool_use", "id": call["id"], "name": call["function"]["name"], "input": input}));
    }
    let refused = message["refusal"].as_str().is_some_and(|r| !r.is_empty());
    let stop_reason = match (refused, choice["finish_reason"].as_str()) {
        (true, _) | (_, Some("content_filter")) => "refusal",
        (_, Some("length")) => "max_tokens",
        (_, Some("tool_calls")) => "tool_use",
        _ if content.iter().any(|b| b["type"] == "tool_use") => "tool_use",
        _ => "end_turn",
    };
    let usage = &body["usage"];
    let tokens = |v: &Json| v.as_u64().unwrap_or(0);
    let cached = tokens(&usage["prompt_tokens_details"]["cached_tokens"]);
    Ok(Response {
        content,
        stop_reason: stop_reason.into(),
        usage: Usage {
            input_tokens: tokens(&usage["prompt_tokens"]).saturating_sub(cached),
            output_tokens: tokens(&usage["completion_tokens"]),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: cached,
        },
        model: body["model"].as_str().unwrap_or_default().to_string(),
    })
}
