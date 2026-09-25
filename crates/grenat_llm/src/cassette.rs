//! Cassettes: model calls recorded once, then replayed (VCR-style), so that
//! tests calling models are deterministic, fast and free.
//!
//! A replayed call is found by its request (the exact JSON body), not by its
//! position: concurrent calls may come in any order.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value as Json, json};

use crate::*;

pub struct Cassette {
    path: PathBuf,
    mode: Mode,
}

enum Mode {
    /// Interactions not replayed yet.
    Replay(Mutex<Vec<(Json, Response)>>),
    /// Real calls, kept to be saved.
    Record { real: Arc<dyn Provider>, calls: Mutex<Vec<(Json, Response)>> },
}

impl Cassette {
    /// Replays `path`.
    pub fn replay(path: &Path) -> Result<Cassette, LlmError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| LlmError::new(format!("cannot read the cassette {}: {e}", path.display())))?;
        let json: Json = serde_json::from_str(&text)
            .map_err(|e| LlmError::new(format!("invalid cassette {}: {e}", path.display())))?;
        let calls = json["interactions"]
            .as_array()
            .ok_or_else(|| LlmError::new(format!("invalid cassette {}: no interactions", path.display())))?
            .iter()
            .map(|i| (i["request"].clone(), response_from_json(&i["response"])))
            .collect();
        Ok(Cassette { path: path.to_path_buf(), mode: Mode::Replay(Mutex::new(calls)) })
    }

    /// Records the calls made through `real`, to be saved at `path`.
    pub fn record(path: &Path, real: Arc<dyn Provider>) -> Cassette {
        Cassette { path: path.to_path_buf(), mode: Mode::Record { real, calls: Mutex::default() } }
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.mode, Mode::Record { .. })
    }

    /// Writes the recorded calls (nothing when replaying).
    pub fn save(&self) -> std::io::Result<()> {
        let Mode::Record { calls, .. } = &self.mode else { return Ok(()) };
        let interactions: Vec<Json> = calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(request, response)| json!({"request": request, "response": response_to_json(response)}))
            .collect();
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&json!({"interactions": interactions})).expect("JSON");
        std::fs::write(&self.path, text + "\n")
    }
}

impl Provider for Cassette {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        let body = request_body(request);
        match &self.mode {
            Mode::Replay(calls) => {
                let mut calls = calls.lock().unwrap_or_else(PoisonError::into_inner);
                let i = calls.iter().position(|(r, _)| *r == body).ok_or_else(|| {
                    LlmError::new(format!(
                        "the cassette {} has no such call (re-record it with GRENAT_RECORD=1)",
                        self.path.display()
                    ))
                })?;
                Ok(calls.remove(i).1)
            }
            Mode::Record { real, calls } => {
                let response = real.complete(request)?;
                calls.lock().unwrap_or_else(PoisonError::into_inner).push((body, response.clone()));
                Ok(response)
            }
        }
    }
}

fn response_to_json(r: &Response) -> Json {
    json!({
        "content": r.content,
        "stop_reason": r.stop_reason,
        "model": r.model,
        "usage": {
            "input_tokens": r.usage.input_tokens,
            "output_tokens": r.usage.output_tokens,
            "cache_creation_input_tokens": r.usage.cache_creation_input_tokens,
            "cache_read_input_tokens": r.usage.cache_read_input_tokens,
        },
    })
}

fn response_from_json(json: &Json) -> Response {
    let tokens = |key: &str| json["usage"][key].as_u64().unwrap_or(0);
    Response {
        content: json["content"].as_array().cloned().unwrap_or_default(),
        stop_reason: json["stop_reason"].as_str().unwrap_or("end_turn").to_string(),
        model: json["model"].as_str().unwrap_or_default().to_string(),
        usage: Usage {
            input_tokens: tokens("input_tokens"),
            output_tokens: tokens("output_tokens"),
            cache_creation_input_tokens: tokens("cache_creation_input_tokens"),
            cache_read_input_tokens: tokens("cache_read_input_tokens"),
        },
    }
}
