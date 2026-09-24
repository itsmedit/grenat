//! Format de l'API Messages : corps des requêtes, lecture des réponses.

use serde_json::{Value as Json, json};

use crate::*;

/// Corps JSON d'une requête `POST /v1/messages`.
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
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema, "strict": true}))
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
    body
}

pub(crate) fn parse_response(body: &Json) -> Result<Response, LlmError> {
    let content = body["content"].as_array().cloned().ok_or_else(|| LlmError::new("réponse sans `content`"))?;
    let usage = &body["usage"];
    let tokens = |field: &str| usage[field].as_u64().unwrap_or(0);
    Ok(Response {
        content,
        stop_reason: body["stop_reason"].as_str().unwrap_or_default().to_string(),
        usage: Usage {
            input_tokens: tokens("input_tokens"),
            output_tokens: tokens("output_tokens"),
            cache_creation_input_tokens: tokens("cache_creation_input_tokens"),
            cache_read_input_tokens: tokens("cache_read_input_tokens"),
        },
        model: body["model"].as_str().unwrap_or_default().to_string(),
    })
}
