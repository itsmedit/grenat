//! The Message Batches API: many requests at once, at half the price,
//! answered within 24 hours (usually minutes).
//!
//! Submit, look at the batch until it has ended, then read its results
//! (JSON Lines, one per request, by `custom_id`).

use serde_json::{Value as Json, json};

use crate::anthropic::Anthropic;
use crate::*;

pub(crate) fn run(client: &Anthropic, requests: &[Request]) -> Result<Vec<Result<Response, LlmError>>, LlmError> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let items: Vec<Json> = requests
        .iter()
        .enumerate()
        .map(|(i, r)| json!({"custom_id": format!("r{i}"), "params": request_body(r)}))
        .collect();
    let base = client.base_url();
    let created =
        expect_json(client.call("POST", &format!("{base}/v1/messages/batches"), Some(&json!({"requests": items}))))?;
    let id = created["id"].as_str().ok_or_else(|| LlmError::new("the batch has no id"))?.to_string();
    let mut batch = created;
    while batch["processing_status"] != "ended" {
        std::thread::sleep(client.poll_interval);
        batch = expect_json(client.call("GET", &format!("{base}/v1/messages/batches/{id}"), None))?;
    }
    let url = batch["results_url"].as_str().ok_or_else(|| LlmError::new("the batch has no results"))?.to_string();
    let (status, lines) = client.call("GET", &url, None).map_err(LlmError::new)?;
    if status != 200 {
        return Err(LlmError::new(format!("batch results: HTTP {status}")));
    }
    let mut results: Vec<Result<Response, LlmError>> =
        (0..requests.len()).map(|_| Err(LlmError::new("no result for this request in the batch"))).collect();
    for line in lines.lines().filter(|l| !l.trim().is_empty()) {
        let item: Json = serde_json::from_str(line).map_err(|e| LlmError::new(format!("invalid batch result: {e}")))?;
        let Some(i) =
            item["custom_id"].as_str().and_then(|c| c.strip_prefix('r')).and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        if i >= results.len() {
            continue;
        }
        let result = &item["result"];
        results[i] = match result["type"].as_str() {
            Some("succeeded") => parse_response(&result["message"]),
            Some("errored") => {
                Err(LlmError::new(result["error"]["error"]["message"].as_str().unwrap_or("error").to_string()))
            }
            Some(other) => Err(LlmError::new(format!("the request was {other}"))),
            None => Err(LlmError::new("invalid batch result")),
        };
    }
    Ok(results)
}

fn expect_json(reply: Result<(u16, String), String>) -> Result<Json, LlmError> {
    let (status, text) = reply.map_err(LlmError::new)?;
    let json: Json = serde_json::from_str(&text).map_err(|e| LlmError::new(format!("unreadable response: {e}")))?;
    if status != 200 {
        let message = json["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(LlmError::new(format!("HTTP {status}: {message}")));
    }
    Ok(json)
}
