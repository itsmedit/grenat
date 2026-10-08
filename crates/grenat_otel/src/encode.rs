//! A batch of spans as an OTLP `ExportTraceServiceRequest`, in the JSON
//! encoding of OTLP/HTTP: field names in lowerCamelCase, trace and span
//! ids in hex (not base64), enums as integers, 64-bit integers as decimal
//! strings (<https://opentelemetry.io/docs/specs/otlp/#json-protobuf-encoding>).

use serde_json::{Value as Json, json};

use crate::span::{Attributes, Kind, Span, Status, Value};

/// The instrumentation scope every span is reported under.
pub const SCOPE: &str = "grenat";

/// The request body for `spans`, all of one resource.
pub fn request(resource: &Attributes, spans: &[Span]) -> Json {
    json!({
        "resourceSpans": [{
            "resource": {"attributes": attributes(resource)},
            "scopeSpans": [{
                "scope": {"name": SCOPE, "version": env!("CARGO_PKG_VERSION")},
                "spans": spans.iter().map(span).collect::<Vec<_>>(),
            }],
        }],
    })
}

fn span(span: &Span) -> Json {
    let mut out = json!({
        "traceId": span.context.trace_id.hex(),
        "spanId": span.context.span_id.hex(),
        "name": span.name,
        "kind": kind(span.kind),
        "startTimeUnixNano": span.start_unix_nano.to_string(),
        "endTimeUnixNano": span.end_unix_nano.to_string(),
        "attributes": attributes(&span.attributes),
        "status": status(&span.status),
    });
    if let Some(parent) = span.parent {
        out["parentSpanId"] = json!(parent.hex());
    }
    if span.context.sampled {
        // bit 0 of the W3C flags, as OTLP's `flags` carries them
        out["flags"] = json!(1);
    }
    if !span.events.is_empty() {
        let events = span.events.iter().map(|event| {
            json!({
                "timeUnixNano": event.time_unix_nano.to_string(),
                "name": event.name,
                "attributes": attributes(&event.attributes),
            })
        });
        out["events"] = Json::Array(events.collect());
    }
    out
}

fn kind(kind: Kind) -> i64 {
    kind as i64
}

fn status(status: &Status) -> Json {
    match status {
        Status::Unset => json!({}),
        Status::Ok => json!({"code": 1}),
        Status::Error(None) => json!({"code": 2}),
        Status::Error(Some(message)) => json!({"code": 2, "message": message}),
    }
}

fn attributes(attributes: &Attributes) -> Json {
    Json::Array(attributes.iter().map(|(key, value)| json!({"key": key, "value": any_value(value)})).collect())
}

/// An `AnyValue`: a double that JSON cannot hold (NaN, infinite) is
/// written as protobuf's JSON mapping says, as a string.
fn any_value(value: &Value) -> Json {
    match value {
        Value::Str(text) => json!({"stringValue": text}),
        Value::Int(n) => json!({"intValue": n.to_string()}),
        Value::Float(x) if x.is_finite() => json!({"doubleValue": x}),
        Value::Float(x) if x.is_nan() => json!({"doubleValue": "NaN"}),
        Value::Float(x) => json!({"doubleValue": if *x > 0.0 { "Infinity" } else { "-Infinity" }}),
        Value::Bool(b) => json!({"boolValue": b}),
        Value::Strs(items) => {
            let values: Vec<Json> = items.iter().map(|item| json!({"stringValue": item})).collect();
            json!({"arrayValue": {"values": values}})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::SpanContext;
    use crate::ids::{SpanId, TraceId};
    use crate::span::SpanStart;

    #[test]
    fn a_span_as_otlp_json() {
        let trace_id = TraceId::parse("5b8efff798038103d269b633813fc60c").unwrap();
        let parent = SpanId::parse("eee19b7ec3c1b173").unwrap();
        let context = SpanContext { trace_id, span_id: SpanId::parse("eee19b7ec3c1b174").unwrap(), sampled: true };
        let start = SpanStart::new("chat claude", Kind::Client)
            .attr("gen_ai.usage.input_tokens", 12_i64)
            .attr("grenat.usage.cost_usd", 0.5)
            .attr("ok", true)
            .attr("gen_ai.response.finish_reasons", vec!["stop".to_string()]);
        let mut span = Span::open(start, context, Some(parent));
        span.start_unix_nano = 1_544_712_660_000_000_000;
        span.end_unix_nano = 1_544_712_661_000_000_000;
        span.fail("LlmError", None);
        span.events[0].time_unix_nano = 1_544_712_660_500_000_000;
        let resource = vec![("service.name".to_string(), Value::from("shop"))];
        let body = request(&resource, &[span]);
        let resource_spans = &body["resourceSpans"][0];
        assert_eq!(
            resource_spans["resource"]["attributes"][0],
            json!({"key": "service.name", "value": {"stringValue": "shop"}})
        );
        assert_eq!(resource_spans["scopeSpans"][0]["scope"]["name"], "grenat");
        let span = &resource_spans["scopeSpans"][0]["spans"][0];
        assert_eq!(span["traceId"], "5b8efff798038103d269b633813fc60c");
        assert_eq!(span["spanId"], "eee19b7ec3c1b174");
        assert_eq!(span["parentSpanId"], "eee19b7ec3c1b173");
        assert_eq!(span["kind"], 3);
        assert_eq!(span["startTimeUnixNano"], "1544712660000000000");
        assert_eq!(span["endTimeUnixNano"], "1544712661000000000");
        assert_eq!(span["attributes"][0]["value"], json!({"intValue": "12"}));
        assert_eq!(span["attributes"][1]["value"], json!({"doubleValue": 0.5}));
        assert_eq!(span["attributes"][2]["value"], json!({"boolValue": true}));
        assert_eq!(span["attributes"][3]["value"], json!({"arrayValue": {"values": [{"stringValue": "stop"}]}}));
        assert_eq!(span["status"], json!({"code": 2, "message": "LlmError"}));
        assert_eq!(span["events"][0]["name"], "exception");
        assert_eq!(span["events"][0]["timeUnixNano"], "1544712660500000000");
    }

    #[test]
    fn a_root_span_has_no_parent_and_numbers_json_cannot_hold_are_strings() {
        let context = SpanContext { trace_id: TraceId::random(), span_id: SpanId::random(), sampled: true };
        let span = Span::open(SpanStart::new("run", Kind::Internal).attr("x", f64::NAN), context, None);
        let body = request(&Vec::new(), &[span]);
        let span = &body["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert!(span.get("parentSpanId").is_none());
        assert_eq!(span["attributes"][0]["value"], json!({"doubleValue": "NaN"}));
        assert_eq!(span["status"], json!({}));
    }
}
