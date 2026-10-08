//! The tracer: starts spans (children of a parent, or roots of new
//! traces) and hands those ended to the exporter.

use std::time::Duration;

use crate::config::Config;
use crate::context::SpanContext;
use crate::exporter::{Batching, Exporter, Stats};
use crate::ids::{SpanId, TraceId};
use crate::span::{Span, SpanStart};
use crate::transport::{Http, Transport};

/// Starts spans and exports them.
pub struct Tracer {
    exporter: Exporter,
    capture_content: bool,
}

impl Tracer {
    /// A tracer exporting over OTLP/HTTP as `config` says; `None` if its
    /// thread cannot start.
    pub fn new(config: &Config) -> Option<Tracer> {
        let transport = Http::new(&config.endpoint, &config.headers, config.timeout);
        Tracer::with_transport(config, Box::new(transport))
    }

    /// A tracer exporting through `transport` (a test's own).
    pub fn with_transport(config: &Config, transport: Box<dyn Transport>) -> Option<Tracer> {
        let batching = Batching { max_queue: config.max_queue, max_batch: config.max_batch, delay: config.delay };
        let exporter = Exporter::start(config.resource.clone(), transport, batching)?;
        Some(Tracer { exporter, capture_content: config.capture_content })
    }

    /// A tracer as the environment configures it (see [`Config::from_env`]);
    /// `None` when nothing is to be exported. A configuration set wrong is
    /// said on standard error, and nothing is exported.
    pub fn from_env() -> Option<Tracer> {
        match Config::from_env(|name| std::env::var(name).ok()) {
            Ok(Some(config)) => Tracer::new(&config),
            Ok(None) => None,
            Err(why) => {
                eprintln!("warning: OpenTelemetry: {why}: no span is exported");
                None
            }
        }
    }

    /// Whether prompts, answers and error messages are exported too.
    pub fn capture_content(&self) -> bool {
        self.capture_content
    }

    /// Starts a span: a child of `parent` (or of the remote parent `start`
    /// carries), else the root of a new trace.
    pub fn start(&self, start: SpanStart, parent: Option<SpanContext>) -> Span {
        let parent = start.remote_parent.or(parent);
        let context = SpanContext {
            trace_id: parent.map_or_else(TraceId::random, |p| p.trace_id),
            span_id: SpanId::random(),
            sampled: true,
        };
        Span::open(start, context, parent.map(|p| p.span_id))
    }

    /// Ends `span` now, and queues it for export.
    pub fn end(&self, mut span: Span) {
        span.close();
        self.exporter.push(span);
    }

    /// Sends what is queued, waiting `timeout` at most.
    pub fn flush(&self, timeout: Duration) -> bool {
        self.exporter.flush(timeout)
    }

    /// Flushes, waiting `timeout` at most, and stops exporting.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        self.exporter.shutdown(timeout)
    }

    pub fn stats(&self) -> Stats {
        self.exporter.stats()
    }
}
