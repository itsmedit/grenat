//! HTTP client for Anthropic's Messages API, with retries.

use std::time::Duration;

use serde_json::Value as Json;

use crate::*;

/// HTTP client for Anthropic's Messages API.
pub struct Anthropic {
    agent: ureq::Agent,
    api_key: String,
    base_url: String,
    /// Delay before the first retry, doubled afterwards (unless `retry-after` says otherwise).
    retry_delay: Duration,
}

pub(crate) const MAX_ATTEMPTS: u32 = 4;

impl Anthropic {
    /// Reads `ANTHROPIC_API_KEY` (and the optional `ANTHROPIC_BASE_URL`).
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| LlmError::new("ANTHROPIC_API_KEY is not set (export ANTHROPIC_API_KEY=sk-ant-…)"))?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into());
        Ok(Anthropic::new(api_key, base_url))
    }

    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(600)))
            .build()
            .into();
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Anthropic { agent, api_key: api_key.into(), base_url, retry_delay: Duration::from_secs(1) }
    }

    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    fn send(&self, body: &Json) -> Result<(u16, Option<u64>, Json), String> {
        let mut request = self
            .agent
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");
        if body.get("fallbacks").is_some() {
            request = request.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        let mut response = request.send_json(body).map_err(|e| format!("connection failed: {e}"))?;
        let status = response.status().as_u16();
        let retry_after =
            response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok());
        let json = response.body_mut().read_json::<Json>().map_err(|e| format!("unreadable response: {e}"))?;
        Ok((status, retry_after, json))
    }
}

impl Provider for Anthropic {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        let body = request_body(request);
        let mut last_error = String::new();
        for attempt in 0..MAX_ATTEMPTS {
            let wait = match self.send(&body) {
                Ok((200, _, json)) => return parse_response(&json),
                Ok((status, retry_after, json)) => {
                    let message = json["error"]["message"].as_str().unwrap_or("unknown error");
                    last_error = format!("HTTP {status}: {message}");
                    // 408, 409, 429, 5xx (including 529 "overloaded"): retry
                    if !(matches!(status, 408 | 409 | 429) || status >= 500) {
                        break;
                    }
                    retry_after.map_or(self.retry_delay * (1 << attempt), Duration::from_secs)
                }
                Err(e) => {
                    last_error = e;
                    self.retry_delay * (1 << attempt)
                }
            };
            if attempt + 1 < MAX_ATTEMPTS {
                std::thread::sleep(wait.min(Duration::from_secs(30)));
            }
        }
        Err(LlmError::new(last_error))
    }
}
