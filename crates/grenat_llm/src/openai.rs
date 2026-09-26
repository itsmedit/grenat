//! HTTP client for OpenAI (its Responses API) and for the providers that
//! speak its Chat Completions: Gemini (Google's compatible endpoint),
//! Mistral, xAI, OpenRouter, Groq, DeepSeek, Together, Ollama.

use std::time::Duration;

use serde_json::Value as Json;

use crate::catalog::{Catalogued, Protocol};
use crate::openai_wire::{chat_body, parse_chat};
use crate::responses_wire::{parse_responses, responses_body};
use crate::*;

pub struct OpenAi {
    agent: ureq::Agent,
    provider: Catalogued,
    /// `None` for a local server that needs none.
    api_key: Option<String>,
    base_url: String,
    retry_delay: Duration,
}

impl OpenAi {
    /// A client of `provider` at `base_url` (its own by default).
    pub fn new(provider: Catalogued, api_key: Option<String>, base_url: Option<&str>) -> OpenAi {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(600)))
            .build()
            .into();
        let base_url = base_url.unwrap_or(provider.base_url).trim_end_matches('/').to_string();
        OpenAi { agent, provider, api_key, base_url, retry_delay: Duration::from_secs(1) }
    }

    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    fn send(&self, path: &str, body: &Json) -> crate::retry::Attempt {
        let mut request = self.agent.post(format!("{}{path}", self.base_url)).header("content-type", "application/json");
        if let Some(key) = &self.api_key {
            request = request.header("authorization", &format!("Bearer {key}"));
        }
        let mut response = request.send_json(body).map_err(|e| format!("connection failed: {e}"))?;
        let status = response.status().as_u16();
        let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok());
        let text = response.body_mut().read_to_string().map_err(|e| format!("unreadable response: {e}"))?;
        let json = serde_json::from_str(&text).unwrap_or(Json::String(text));
        Ok((status, retry_after, json))
    }
}

impl Provider for OpenAi {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        if self.provider.protocol == Protocol::Responses {
            let body = responses_body(request, &self.provider)?;
            return crate::retry::with_retries(self.retry_delay, || self.send("/responses", &body), parse_responses);
        }
        let body = chat_body(request, &self.provider)?;
        crate::retry::with_retries(self.retry_delay, || self.send("/chat/completions", &body), parse_chat)
    }
}
