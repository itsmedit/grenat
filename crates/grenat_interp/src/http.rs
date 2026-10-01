//! HTTP transport: one request, one reply, nothing about the language.

mod proxy;

use std::io::Read;
use std::time::Duration;

pub(crate) use proxy::{ProxyChoice, validate as validate_proxy, without_credentials};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub timeout: Duration,
    pub proxy: ProxyChoice,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HttpReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Sends `request`; an error is a failure to get any answer (a status
/// such as 404 is an answer, an unreachable proxy is not).
pub(crate) fn send(request: &HttpRequest) -> Result<HttpReply, String> {
    let (status, headers, mut response) = run(request)?;
    let body = response.body_mut().read_to_string().map_err(|e| format!("unreadable body: {e}"))?;
    Ok(HttpReply { status, headers, body })
}

/// The bytes `request` gets, `max_bytes` at most: a larger body (by its
/// `Content-Length`, else by what arrives) is read no further and not kept.
pub(crate) fn download(request: &HttpRequest, max_bytes: usize) -> Result<Download, String> {
    let (status, headers, mut response) = run(request)?;
    let announced = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<u64>().ok());
    if status == 200 && announced.is_some_and(|size| size > max_bytes as u64) {
        return Ok(Download { status, headers, body: Vec::new(), too_large: true });
    }
    let mut body = Vec::new();
    let reader = response.body_mut().as_reader();
    reader.take(max_bytes as u64 + 1).read_to_end(&mut body).map_err(|e| format!("unreadable body: {e}"))?;
    let too_large = body.len() > max_bytes;
    if too_large {
        body.clear();
    }
    Ok(Download { status, headers, body, too_large })
}

/// What [`download`] got.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Download {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Over the limit: nothing kept.
    pub too_large: bool,
}

/// An answer: its status, its headers, its body unread.
type Answer = (u16, Vec<(String, String)>, ureq::http::Response<ureq::Body>);

/// The answer to `request`.
fn run(request: &HttpRequest) -> Result<Answer, String> {
    let proxy = proxy::resolve(&request.proxy, &request.url, |name| std::env::var(name).ok())?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(request.timeout))
        .proxy(proxy)
        .build()
        .into();
    let mut builder = ureq::http::Request::builder().method(request.method).uri(&request.url);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    let body = request.body.clone().unwrap_or_default().into_bytes();
    let built = builder.body(body).map_err(|e| format!("invalid request: {e}"))?;
    let response = agent.run(built).map_err(|e| e.to_string())?;
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or_default().to_string()))
        .collect();
    let status = response.status().as_u16();
    Ok((status, headers, response))
}

/// `url` with `params` as its query string, percent-encoded.
pub(crate) fn with_query(url: &str, params: &[(String, String)]) -> String {
    if params.is_empty() {
        return url.to_string();
    }
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect();
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}{}", query.join("&"))
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
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
    use super::{host, with_query};

    #[test]
    fn queries_are_encoded() {
        let params = [("q".to_string(), "rust & grenat é".to_string()), ("n".to_string(), "5".to_string())];
        assert_eq!(with_query("https://x.io/s", &params), "https://x.io/s?q=rust%20%26%20grenat%20%C3%A9&n=5");
        assert_eq!(with_query("https://x.io/s?a=1", &params[1..]), "https://x.io/s?a=1&n=5");
    }

    #[test]
    fn hosts() {
        assert_eq!(host("https://api.github.com/repos/x"), Some("api.github.com"));
        assert_eq!(host("http://localhost:8080?q=1"), Some("localhost"));
        assert_eq!(host("https://user:pw@example.com"), Some("example.com"));
        assert_eq!(host("ftp://x"), None);
        assert_eq!(host("https://"), None);
    }
}
