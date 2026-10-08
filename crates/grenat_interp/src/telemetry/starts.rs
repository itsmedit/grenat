//! How the spans of serving and of the outside world start, named and
//! described as OpenTelemetry's conventions say where they say something:
//!
//! - a request served: `SERVER`, `GET` until its route is known (then
//!   `GET /tickets/:id`), `http.request.method`, `http.route`,
//!   `http.response.status_code`; continuing the trace of a `traceparent`
//!   received;
//! - a trigger's run: `schedule every 300s`, `email imap.acme.com/INBOX`,
//!   `file inbox/*.pdf` (`grenat.trigger`, `grenat.trigger.label`);
//! - an `Http` request: `CLIENT`, `GET`, `http.request.method`,
//!   `server.address`, `server.port` (never the URL: it may hold a secret,
//!   its path and query data);
//! - an SSH call: `CLIENT`, `ssh run`, `server.address`, `grenat.ssh.operation`.

use grenat_otel::{Kind, SpanContext, SpanStart};

/// A request served, child of the caller's span if it sent `traceparent`.
pub(crate) fn request_span(method: &str, headers: &[(String, String)]) -> SpanStart {
    let parent = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("traceparent"))
        .and_then(|(_, value)| SpanContext::from_traceparent(value));
    SpanStart::new(method, Kind::Server).attr("http.request.method", method).remote_parent(parent)
}

/// A trigger's run: `kind` is `schedule`, `email` or `file`.
pub(crate) fn trigger_span(kind: &str, label: &str) -> SpanStart {
    SpanStart::new(format!("{kind} {label}"), Kind::Internal)
        .attr("grenat.trigger", kind)
        .attr("grenat.trigger.label", label)
}

/// An `Http` request to `host` (`url` gives its port).
pub(crate) fn http_span(method: &str, url: &str, host: &str) -> SpanStart {
    let port = port(url);
    SpanStart::new(method, Kind::Client)
        .attr("http.request.method", method)
        .attr("server.address", host)
        .attr("server.port", i64::from(port))
}

/// An SSH call (`run`, `upload`, `list`…) to `host`.
pub(crate) fn ssh_span(operation: &str, host: &str) -> SpanStart {
    SpanStart::new(format!("ssh {operation}"), Kind::Client)
        .attr("grenat.ssh.operation", operation)
        .attr("server.address", host)
}

/// The port of an http(s) URL: given, else the scheme's.
fn port(url: &str) -> u16 {
    let (rest, default) = match url.strip_prefix("https://") {
        Some(rest) => (rest, 443),
        None => (url.strip_prefix("http://").unwrap_or(url), 80),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    // an IPv6 literal's colons are inside its brackets
    let after_host = host_port.rsplit(']').next().unwrap_or_default();
    after_host.rsplit_once(':').and_then(|(_, port)| port.parse().ok()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_given_or_by_scheme() {
        assert_eq!(port("https://api.github.com/repos"), 443);
        assert_eq!(port("http://localhost:8080/x?y=1"), 8080);
        assert_eq!(port("http://user:pw@example.com/"), 80);
        assert_eq!(port("http://[::1]:9000/"), 9000);
        assert_eq!(port("http://[::1]/"), 80);
    }

    #[test]
    fn a_request_continues_the_trace_it_was_given() {
        let headers =
            [("TraceParent".to_string(), "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string())];
        let start = request_span("POST", &headers);
        assert_eq!(start.remote_parent.unwrap().span_id.hex(), "00f067aa0ba902b7");
        assert!(request_span("GET", &[("traceparent".into(), "garbage".into())]).remote_parent.is_none());
    }
}
