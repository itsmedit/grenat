//! HTTP transport: one request, one reply, nothing about the language.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HttpReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Sends `request`; an error is a failure to get any answer (a status
/// such as 404 is an answer).
pub(crate) fn send(request: &HttpRequest) -> Result<HttpReply, String> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(request.timeout)).build().into();
    let mut builder = ureq::http::Request::builder().method(request.method).uri(&request.url);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    let body = request.body.clone().unwrap_or_default().into_bytes();
    let built = builder.body(body).map_err(|e| format!("invalid request: {e}"))?;
    let mut response = agent.run(built).map_err(|e| e.to_string())?;
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or_default().to_string()))
        .collect();
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().map_err(|e| format!("unreadable body: {e}"))?;
    Ok(HttpReply { status, headers, body })
}

/// The host of `url` (`https://api.github.com:443/x` → `api.github.com`).
pub(crate) fn host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::host;

    #[test]
    fn hosts() {
        assert_eq!(host("https://api.github.com/repos/x"), Some("api.github.com"));
        assert_eq!(host("http://localhost:8080?q=1"), Some("localhost"));
        assert_eq!(host("https://user:pw@example.com"), Some("example.com"));
        assert_eq!(host("ftp://x"), None);
        assert_eq!(host("https://"), None);
    }
}
