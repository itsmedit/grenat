//! OpenTelemetry traces, exported over OTLP/HTTP in its JSON encoding:
//! what lets a team follow its agents' latency, errors and costs in
//! Datadog, Grafana or Honeycomb. Nothing here knows the Grenat language.
//!
//! **Configuration** ([`Config`]): the standard `OTEL_*` environment
//! variables; without an endpoint, nothing is exported.
//!
//! **Spans** ([`Span`], [`SpanStart`]): a name, a kind, attributes, an
//! `exception` event and a status; their ids are random ([`TraceId`],
//! [`SpanId`]). A span's [`SpanContext`] makes its children, in this
//! process or in another one through the W3C `traceparent` header.
//!
//! **The tracer** ([`Tracer`]): starts spans and hands those ended to
//! **the exporter** ([`Exporter`]), which batches them on a thread of its
//! own, through a bounded queue — a span that finds it full is dropped and
//! counted ([`Stats`]) — and never makes the program wait, whatever the
//! collector does. Batches are encoded as OTLP JSON ([`encode`]) and sent
//! by a [`Transport`] (OTLP/HTTP, or a test's own).

mod config;
mod context;
pub mod encode;
mod exporter;
mod ids;
mod span;
mod tracer;
mod transport;

pub use config::Config;
pub use context::SpanContext;
pub use exporter::{Batching, Exporter, Stats};
pub use ids::{SpanId, TraceId};
pub use span::{Attributes, Event, Kind, MAX_TEXT, Span, SpanStart, Status, Value};
pub use tracer::Tracer;
pub use transport::{Http, Transport};
