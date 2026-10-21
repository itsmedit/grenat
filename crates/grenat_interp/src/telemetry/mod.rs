//! OpenTelemetry spans (see `grenat_otel`): what a run does, as a team
//! follows it in Datadog, Grafana or Honeycomb.
//!
//! A span is opened around each operation worth following — a request, a
//! trigger's run, a job, a workflow and its steps, an agent's `ask`, a
//! model call, a tool call, an `Http` request, an SSH call — by
//! [`Interp::traced`], which closes it when the work returns and records
//! its error, if it raised one. The spans a task opens are nested in the
//! order they run; a task started by another one (`parallel_map`, `tell`,
//! `race`…) starts its spans as children of the span its parent was in.
//!
//! What spans carry is chosen not to leak: names, ids, sizes, counts,
//! tokens and costs — never a secret, a prompt's text, a model's answer or
//! anything untrusted (a request's path, a file's name, a message's
//! subject). An error is its type; its message only when the content is
//! captured (`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`),
//! with the credentials known to the run blanked out.
//!
//! Without a tracer (no `OTEL_EXPORTER_OTLP_*ENDPOINT`, and always in
//! tests), nothing here costs more than a check.

mod genai;
mod redact;
mod starts;

use std::time::Duration;

use grenat_otel::{SpanContext, SpanStart, Value};

use crate::prelude::*;

pub(crate) use genai::{chat_span, embedding_span, transcription_span};
pub(crate) use starts::{http_span, request_span, ssh_span, trigger_span};

/// How long the end of a run waits for its spans to be sent, at most.
const FLUSH_AT_EXIT: Duration = Duration::from_secs(5);

impl<'p> Interp<'p> {
    /// Whether spans are being exported.
    pub(crate) fn tracing(&self) -> bool {
        self.tracer.is_some()
    }

    /// The span running now in this task (else the one that started it):
    /// the parent of the next span.
    pub(crate) fn span_context(&self) -> Option<SpanContext> {
        self.spans.last().map(|span| span.context).or(self.trace_parent)
    }

    /// Runs `work` in a span: `start` describes it (built only when spans
    /// are exported). An error `work` raises is recorded on it — but a job
    /// suspended for a human, which is no failure.
    pub(crate) fn traced<T>(
        &mut self,
        start: impl FnOnce(&Self) -> SpanStart,
        work: impl FnOnce(&mut Self) -> Result<T, Ctrl<'p>>,
    ) -> Result<T, Ctrl<'p>> {
        let Some(tracer) = self.tracer.clone() else { return work(self) };
        let span = tracer.start(start(self), self.span_context());
        let depth = self.spans.len();
        self.spans.push(span);
        let result = work(self);
        if let Err(Ctrl::Raise(error)) = &result
            && &*error.ty != crate::eval::approvals::SUSPENDED
        {
            self.span_error(&error.ty, &error.message);
        }
        // the spans `work` opened are closed by now
        self.spans.truncate(depth + 1);
        if let Some(span) = self.spans.pop() {
            tracer.end(span);
        }
        result
    }

    /// Sets an attribute of the span running now, if there is one.
    pub(crate) fn span_set(&mut self, key: &str, value: impl Into<Value>) {
        if let Some(span) = self.spans.last_mut() {
            span.set(key, value);
        }
    }

    /// Renames the span running now (a request, once its route is known).
    pub(crate) fn span_rename(&mut self, name: impl Into<String>) {
        if let Some(span) = self.spans.last_mut() {
            span.rename(name);
        }
    }

    /// Records an error on the span running now: its type, and its message
    /// only when the content is captured (secrets blanked out).
    pub(crate) fn span_error(&mut self, ty: &str, message: &str) {
        let Some(tracer) = self.tracer.clone() else { return };
        let message = tracer.capture_content().then(|| self.redact(message));
        if let Some(span) = self.spans.last_mut() {
            span.fail(ty, message.as_deref());
        }
    }

    /// Marks the span running now failed, with no exception (a status
    /// `500` answered).
    pub(crate) fn span_failed(&mut self, ty: &str) {
        if let Some(span) = self.spans.last_mut() {
            span.fail_quietly(ty);
        }
    }

    /// At the end of a run: sends the spans still queued (waiting a few
    /// seconds at most), and says what could not be exported.
    pub(crate) fn finish_telemetry(&self) {
        let Some(tracer) = &self.tracer else { return };
        let flushed = tracer.shutdown(FLUSH_AT_EXIT);
        let stats = tracer.stats();
        let mut lost = Vec::new();
        let full = stats.dropped - stats.rejected;
        if full > 0 {
            lost.push(format!("{full} dropped (the queue was full)"));
        }
        if stats.rejected > 0 {
            let why = stats.rejection.as_deref().unwrap_or("no reason given");
            lost.push(format!("{} rejected by the collector ({why})", stats.rejected));
        }
        if stats.failed > 0 {
            let why = stats.last_error.as_deref().unwrap_or("unknown error");
            lost.push(format!("{} not exported ({why})", stats.failed));
        }
        if !flushed {
            lost.push(format!("the rest not sent within {}s", FLUSH_AT_EXIT.as_secs()));
        }
        if !lost.is_empty() {
            self.write_err(&format!("warning: OpenTelemetry: spans {}\n", lost.join(", ")));
        }
    }
}
