//! How a batch reaches the collector: one OTLP/HTTP `POST` of its JSON,
//! bounded by a timeout. Behind a trait, so that tests can stand in.
//!
//! A redirect is never followed: the configured headers carry API keys
//! (`x-honeycomb-team`, `dd-api-key`), which a redirect would hand to
//! whatever host it names. A collector that cannot be reached (refused,
//! reset, not found) is worth trying again; one that took the whole
//! timeout may have taken the batch, and is not.

use std::time::{Duration, SystemTime};

use crate::answer::{self, Export};

/// The most of an accepted answer read (its partial success).
const MAX_ANSWER: u64 = 64 * 1024;

/// Sends one encoded batch, and says what became of it.
pub trait Transport: Send + Sync {
    fn send(&self, body: &[u8]) -> Export;
}

/// OTLP/HTTP: `POST <endpoint>` with `Content-Type: application/json`.
pub struct Http {
    endpoint: String,
    headers: Vec<(String, String)>,
    agent: ureq::Agent,
}

impl Http {
    pub fn new(endpoint: &str, headers: &[(String, String)], timeout: Duration) -> Http {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(timeout))
            // a 3xx is returned as it is, never followed with the API keys
            .max_redirects(0)
            // the environment's proxy, if any (`https_proxy`…), as ureq reads it
            .build()
            .into();
        Http { endpoint: endpoint.to_string(), headers: headers.to_vec(), agent }
    }
}

impl Transport for Http {
    /// What the collector answered (see [`answer::classify`]); an error
    /// names neither the headers (API keys) nor the body.
    fn send(&self, body: &[u8]) -> Export {
        let mut request = ureq::http::Request::builder()
            .method("POST")
            .uri(&self.endpoint)
            .header("Content-Type", "application/json");
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        let Ok(request) = request.body(body.to_vec()) else {
            return Export::Failed("invalid OTLP request (a header?)".into());
        };
        let mut response = match self.agent.run(request) {
            Ok(response) => response,
            Err(e @ ureq::Error::Timeout(_)) => return Export::Failed(format!("collector unreachable: {e}")),
            Err(e) => return Export::Retry { why: format!("collector unreachable: {e}"), retry_after: None },
        };
        let status = response.status().as_u16();
        let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
        let body = if (200..300).contains(&status) {
            response.body_mut().with_config().limit(MAX_ANSWER).read_to_vec().unwrap_or_default()
        } else {
            Vec::new()
        };
        answer::classify(status, retry_after.as_deref(), &body, SystemTime::now())
    }
}
