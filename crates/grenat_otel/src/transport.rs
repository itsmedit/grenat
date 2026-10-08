//! How a batch reaches the collector: one OTLP/HTTP `POST` of its JSON,
//! bounded by a timeout. Behind a trait, so that tests can stand in.

use std::time::Duration;

/// Sends one encoded batch; an error says why it was not accepted.
pub trait Transport: Send + Sync {
    fn send(&self, body: Vec<u8>) -> Result<(), String>;
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
            // the environment's proxy, if any (`https_proxy`…), as ureq reads it
            .build()
            .into();
        Http { endpoint: endpoint.to_string(), headers: headers.to_vec(), agent }
    }
}

impl Transport for Http {
    /// A `2xx` is accepted (a partial success included: OTLP says not to
    /// send it again); any other answer, or none, is an error that names
    /// neither the headers (API keys) nor the body.
    fn send(&self, body: Vec<u8>) -> Result<(), String> {
        let mut request = ureq::http::Request::builder()
            .method("POST")
            .uri(&self.endpoint)
            .header("Content-Type", "application/json");
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        let request = request.body(body).map_err(|_| "invalid OTLP request (a header?)".to_string())?;
        let response = self.agent.run(request).map_err(|e| format!("collector unreachable: {e}"))?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) { Ok(()) } else { Err(format!("collector answered {status}")) }
    }
}
