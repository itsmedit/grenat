//! Where and how spans are exported, read from the standard OpenTelemetry
//! environment variables
//! (<https://opentelemetry.io/docs/specs/otel/protocol/exporter/>,
//! <https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/>):
//!
//! - `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` (used as is), else
//!   `OTEL_EXPORTER_OTLP_ENDPOINT` (`/v1/traces` appended); neither set,
//!   nothing is exported;
//! - `OTEL_EXPORTER_OTLP_TRACES_HEADERS`, else `OTEL_EXPORTER_OTLP_HEADERS`
//!   (`key=value,key2=value2`, values percent-decoded);
//! - `OTEL_EXPORTER_OTLP_TRACES_TIMEOUT`, else `OTEL_EXPORTER_OTLP_TIMEOUT`
//!   (milliseconds, 10 s by default);
//! - `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` (the resource);
//! - `OTEL_SDK_DISABLED=true`, `OTEL_TRACES_EXPORTER=none`: nothing exported;
//! - `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL`, else `OTEL_EXPORTER_OTLP_PROTOCOL`:
//!   spans always go as `http/json`; `grpc` is refused;
//! - `OTEL_BSP_MAX_QUEUE_SIZE`, `OTEL_BSP_SCHEDULE_DELAY` (a millisecond
//!   at least: `0` is taken as `1`), `OTEL_BSP_MAX_EXPORT_BATCH_SIZE`: the
//!   batching;
//! - `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`: prompts,
//!   answers and error messages are exported too (off by default).

use std::time::Duration;

use crate::span::{Attributes, Value};

/// How spans are exported.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The URL spans are posted to (`http://collector:4318/v1/traces`).
    pub endpoint: String,
    /// Sent with every export (an API key…): never logged.
    pub headers: Vec<(String, String)>,
    /// How long one export may take.
    pub timeout: Duration,
    /// What every span is about: `service.name` first.
    pub resource: Attributes,
    /// Spans kept waiting at most; those beyond are dropped (and counted).
    pub max_queue: usize,
    /// Spans sent together at most.
    pub max_batch: usize,
    /// How long spans wait before they are sent.
    pub delay: Duration,
    /// Prompts, answers and error messages exported too.
    pub capture_content: bool,
}

impl Config {
    /// A configuration exporting to `endpoint` (the full URL), with the
    /// defaults: what tests and embedders start from.
    pub fn new(endpoint: impl Into<String>, service: &str) -> Config {
        Config {
            endpoint: endpoint.into(),
            headers: Vec::new(),
            timeout: Duration::from_secs(10),
            resource: resource(service, &[]),
            max_queue: 2048,
            max_batch: 512,
            delay: Duration::from_secs(5),
            capture_content: false,
        }
    }

    /// The configuration the environment gives (`var` reads a variable):
    /// `Ok(None)` when no endpoint is set or export is disabled, an error
    /// when it is set wrong.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Option<Config>, String> {
        let var = |name: &str| var(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        if var("OTEL_SDK_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
            return Ok(None);
        }
        if let Some(exporters) = var("OTEL_TRACES_EXPORTER")
            && !exporters.split(',').any(|e| e.trim() == "otlp")
        {
            return Ok(None);
        }
        let endpoint = match (var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"), var("OTEL_EXPORTER_OTLP_ENDPOINT")) {
            (Some(traces), _) => traces,
            (None, Some(base)) => format!("{}/v1/traces", base.trim_end_matches('/')),
            (None, None) => return Ok(None),
        };
        if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
            // not echoed: an endpoint may carry a key
            return Err("the OTLP endpoint must be an http(s) URL (http://collector:4318)".into());
        }
        let protocol = var("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL").or_else(|| var("OTEL_EXPORTER_OTLP_PROTOCOL"));
        if protocol.as_deref() == Some("grpc") {
            return Err("OTLP over gRPC is not supported: use OTLP/HTTP (port 4318), sent as http/json".into());
        }
        let headers = match var("OTEL_EXPORTER_OTLP_TRACES_HEADERS").or_else(|| var("OTEL_EXPORTER_OTLP_HEADERS")) {
            Some(list) => pairs(&list).map_err(|e| format!("OTEL_EXPORTER_OTLP_HEADERS: {e}"))?,
            None => Vec::new(),
        };
        let millis = |name: &str| -> Result<Option<Duration>, String> {
            var(name)
                .map(|v| {
                    v.parse::<u64>().map(Duration::from_millis).map_err(|_| format!("{name}: milliseconds, got {v:?}"))
                })
                .transpose()
        };
        let count = |name: &str, default: usize| -> Result<usize, String> {
            var(name).map_or(Ok(default), |v| {
                v.parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("{name}: a positive number, got {v:?}"))
            })
        };
        let given = match var("OTEL_RESOURCE_ATTRIBUTES") {
            Some(list) => pairs(&list).map_err(|e| format!("OTEL_RESOURCE_ATTRIBUTES: {e}"))?,
            None => Vec::new(),
        };
        let service = var("OTEL_SERVICE_NAME")
            .or_else(|| given.iter().find(|(k, _)| k == "service.name").map(|(_, v)| v.clone()))
            .unwrap_or_else(|| "unknown_service:grenat".to_string());
        let defaults = Config::new(endpoint, &service);
        let max_queue = count("OTEL_BSP_MAX_QUEUE_SIZE", defaults.max_queue)?;
        Ok(Some(Config {
            headers,
            timeout: millis("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT")?
                .or(millis("OTEL_EXPORTER_OTLP_TIMEOUT")?)
                .unwrap_or(defaults.timeout),
            resource: resource(&service, &given),
            max_queue,
            max_batch: count("OTEL_BSP_MAX_EXPORT_BATCH_SIZE", defaults.max_batch)?.min(max_queue),
            delay: millis("OTEL_BSP_SCHEDULE_DELAY")?.unwrap_or(defaults.delay).max(crate::exporter::MIN_DELAY),
            capture_content: var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT")
                .is_some_and(|v| v.eq_ignore_ascii_case("true")),
            ..defaults
        }))
    }
}

/// The resource: `service.name`, the SDK's attributes, then those given
/// (but their `service.name`, which `service` already is).
fn resource(service: &str, given: &[(String, String)]) -> Attributes {
    let mut attributes: Attributes = vec![
        ("service.name".into(), Value::from(service)),
        ("telemetry.sdk.name".into(), Value::from("grenat")),
        ("telemetry.sdk.language".into(), Value::from("rust")),
        ("telemetry.sdk.version".into(), Value::from(env!("CARGO_PKG_VERSION"))),
    ];
    for (key, value) in given {
        if key != "service.name" {
            attributes.push((key.clone(), Value::from(value.as_str())));
        }
    }
    attributes
}

/// `key=value,key2=value2`, keys and values trimmed, values percent-decoded.
fn pairs(list: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for item in list.split(',').map(str::trim).filter(|item| !item.is_empty()) {
        let Some((key, value)) = item.split_once('=') else {
            // never echoed: it may be an API key
            return Err("an entry without `=`: expected key=value,key2=value2".into());
        };
        let key = key.trim();
        if key.is_empty() {
            return Err("an empty key".into());
        }
        out.push((key.to_string(), percent_decode(value.trim())));
    }
    Ok(out)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from(vars: &[(&str, &str)]) -> Result<Option<Config>, String> {
        let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Config::from_env(|name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    #[test]
    fn nothing_is_exported_without_an_endpoint_or_when_disabled() {
        assert_eq!(from(&[]), Ok(None));
        assert_eq!(from(&[("OTEL_SERVICE_NAME", "shop")]), Ok(None));
        let endpoint = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318");
        assert_eq!(from(&[endpoint, ("OTEL_SDK_DISABLED", "true")]), Ok(None));
        assert_eq!(from(&[endpoint, ("OTEL_TRACES_EXPORTER", "none")]), Ok(None));
        assert!(from(&[endpoint, ("OTEL_TRACES_EXPORTER", "otlp")]).unwrap().is_some());
    }

    #[test]
    fn the_endpoint_of_traces_wins_and_the_base_gets_its_path() {
        let base = from(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318/")]).unwrap().unwrap();
        assert_eq!(base.endpoint, "http://collector:4318/v1/traces");
        let traces = from(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318"),
            ("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "https://api.honeycomb.io/v1/traces"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(traces.endpoint, "https://api.honeycomb.io/v1/traces");
        assert!(from(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "collector:4318")]).is_err());
        assert!(
            from(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4317"), ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc")]).is_err()
        );
    }

    #[test]
    fn headers_resource_timeout_and_content_capture() {
        let config = from(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318"),
            ("OTEL_EXPORTER_OTLP_HEADERS", "x-honeycomb-team=abc%3D%3D, x-other = 1"),
            ("OTEL_RESOURCE_ATTRIBUTES", "service.name=ignored,deployment.environment.name=prod"),
            ("OTEL_SERVICE_NAME", "support-agents"),
            ("OTEL_EXPORTER_OTLP_TIMEOUT", "2500"),
            ("OTEL_BSP_MAX_QUEUE_SIZE", "100"),
            ("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(config.headers, [("x-honeycomb-team".into(), "abc==".into()), ("x-other".into(), "1".into())]);
        assert_eq!(config.resource[0], ("service.name".into(), Value::from("support-agents")));
        assert!(config.resource.contains(&("deployment.environment.name".into(), Value::from("prod"))));
        assert_eq!(config.resource.iter().filter(|(k, _)| k == "service.name").count(), 1);
        assert_eq!(config.timeout, Duration::from_millis(2500));
        assert_eq!((config.max_queue, config.max_batch), (100, 100));
        assert!(config.capture_content);
        let plain = from(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318")]).unwrap().unwrap();
        assert!(!plain.capture_content);
        assert_eq!(plain.resource[0], ("service.name".into(), Value::from("unknown_service:grenat")));
        assert!(
            from(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318"), ("OTEL_EXPORTER_OTLP_HEADERS", "novalue")])
                .is_err()
        );
        assert!(
            from(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318"), ("OTEL_EXPORTER_OTLP_TIMEOUT", "soon")]).is_err()
        );
    }

    #[test]
    fn a_delay_of_zero_is_a_millisecond() {
        let endpoint = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318");
        let zero = from(&[endpoint, ("OTEL_BSP_SCHEDULE_DELAY", "0")]).unwrap().unwrap();
        assert_eq!(zero.delay, Duration::from_millis(1));
        let given = from(&[endpoint, ("OTEL_BSP_SCHEDULE_DELAY", "250")]).unwrap().unwrap();
        assert_eq!(given.delay, Duration::from_millis(250));
    }
}
