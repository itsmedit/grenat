//! A span: an operation with a name, a kind, a start and an end, its
//! attributes, its events (an exception) and its status.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::context::SpanContext;
use crate::ids::SpanId;

/// The longest text an attribute keeps (in bytes, cut at a character).
pub const MAX_TEXT: usize = 16 * 1024;

/// A span's kind, numbered as in OTLP's `SpanKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Internal = 1,
    Server = 2,
    Client = 3,
    Producer = 4,
    Consumer = 5,
}

/// An attribute's value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Strs(Vec<String>),
}

impl From<&str> for Value {
    fn from(text: &str) -> Value {
        Value::Str(cut(text.to_string()))
    }
}

impl From<String> for Value {
    fn from(text: String) -> Value {
        Value::Str(cut(text))
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Value {
        Value::Int(n)
    }
}

impl From<u64> for Value {
    fn from(n: u64) -> Value {
        Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }
}

impl From<usize> for Value {
    fn from(n: usize) -> Value {
        Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }
}

impl From<f64> for Value {
    fn from(x: f64) -> Value {
        Value::Float(x)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

impl From<Vec<String>> for Value {
    fn from(items: Vec<String>) -> Value {
        Value::Strs(items.into_iter().map(cut).collect())
    }
}

/// `text`, cut to [`MAX_TEXT`] bytes.
fn cut(mut text: String) -> String {
    if text.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

pub type Attributes = Vec<(String, Value)>;

/// Something that happened during a span (an exception).
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub name: String,
    pub time_unix_nano: u64,
    pub attributes: Attributes,
}

/// A span's outcome, numbered as in OTLP's `StatusCode`.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Unset,
    Ok,
    Error(Option<String>),
}

/// What a span starts with: its name, kind and first attributes, and the
/// parent another process gave (a `traceparent` received), if any.
#[derive(Debug, Clone)]
pub struct SpanStart {
    pub name: String,
    pub kind: Kind,
    pub attributes: Attributes,
    pub remote_parent: Option<SpanContext>,
}

impl SpanStart {
    pub fn new(name: impl Into<String>, kind: Kind) -> SpanStart {
        SpanStart { name: name.into(), kind, attributes: Vec::new(), remote_parent: None }
    }

    pub fn attr(mut self, key: &str, value: impl Into<Value>) -> SpanStart {
        self.attributes.push((key.to_string(), value.into()));
        self
    }

    pub fn remote_parent(mut self, parent: Option<SpanContext>) -> SpanStart {
        self.remote_parent = parent;
        self
    }
}

/// A span, open until [`crate::Tracer::end`].
#[derive(Debug, Clone)]
pub struct Span {
    pub context: SpanContext,
    pub parent: Option<SpanId>,
    pub name: String,
    pub kind: Kind,
    pub start_unix_nano: u64,
    pub end_unix_nano: u64,
    pub attributes: Attributes,
    pub events: Vec<Event>,
    pub status: Status,
    started: Instant,
}

impl Span {
    pub(crate) fn open(start: SpanStart, context: SpanContext, parent: Option<SpanId>) -> Span {
        Span {
            context,
            parent,
            name: start.name,
            kind: start.kind,
            start_unix_nano: now_unix_nano(),
            end_unix_nano: 0,
            attributes: start.attributes,
            events: Vec::new(),
            status: Status::Unset,
            started: Instant::now(),
        }
    }

    /// Ends it now: its end is measured on a monotonic clock from its start.
    pub(crate) fn close(&mut self) {
        let elapsed = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.end_unix_nano = self.start_unix_nano.saturating_add(elapsed);
    }

    pub fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Sets `key`, replacing its value if it has one.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        let value = value.into();
        match self.attributes.iter_mut().find(|(k, _)| k == key) {
            Some((_, old)) => *old = value,
            None => self.attributes.push((key.to_string(), value)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Adds `n` to the integer `key` (0 when unset).
    pub fn add_int(&mut self, key: &str, n: i64) {
        let total = match self.get(key) {
            Some(Value::Int(old)) => old.saturating_add(n),
            _ => n,
        };
        self.set(key, total);
    }

    /// Adds `x` to the number `key` (0 when unset).
    pub fn add_float(&mut self, key: &str, x: f64) {
        let total = match self.get(key) {
            Some(Value::Float(old)) => old + x,
            _ => x,
        };
        self.set(key, total);
    }

    /// Marks it failed: the `error.type` attribute, an `exception` event
    /// (its type; its message only when given) and the error status.
    pub fn fail(&mut self, ty: &str, message: Option<&str>) {
        self.set("error.type", ty);
        let mut attributes = vec![("exception.type".to_string(), Value::from(ty))];
        if let Some(message) = message {
            attributes.push(("exception.message".to_string(), Value::from(message)));
        }
        self.events.push(Event { name: "exception".into(), time_unix_nano: now_unix_nano(), attributes });
        self.status = Status::Error(Some(ty.to_string()));
    }

    /// Marks it failed without an exception (a status `500`, `404`…).
    pub fn fail_quietly(&mut self, ty: &str) {
        self.set("error.type", ty);
        self.status = Status::Error(None);
    }
}

pub(crate) fn now_unix_nano() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::TraceId;

    fn span() -> Span {
        let context = SpanContext { trace_id: TraceId::random(), span_id: SpanId::random(), sampled: true };
        Span::open(SpanStart::new("work", Kind::Internal).attr("a", 1_i64), context, None)
    }

    #[test]
    fn attributes_are_set_once_and_summed() {
        let mut span = span();
        span.set("a", 2_i64);
        span.add_int("tokens", 10);
        span.add_int("tokens", 5);
        span.add_float("cost", 0.25);
        span.add_float("cost", 0.5);
        assert_eq!(span.get("a"), Some(&Value::Int(2)));
        assert_eq!(span.get("tokens"), Some(&Value::Int(15)));
        assert_eq!(span.get("cost"), Some(&Value::Float(0.75)));
        assert_eq!(span.attributes.len(), 3);
    }

    #[test]
    fn a_failure_is_an_exception_event_and_an_error_status() {
        let mut span = span();
        span.fail("HttpError", None);
        assert_eq!(span.status, Status::Error(Some("HttpError".into())));
        assert_eq!(span.events[0].name, "exception");
        assert_eq!(span.events[0].attributes, vec![("exception.type".to_string(), Value::from("HttpError"))]);
        span.close();
        assert!(span.end_unix_nano >= span.start_unix_nano);
    }

    #[test]
    fn long_texts_are_cut_at_a_character() {
        let Value::Str(text) = Value::from("é".repeat(MAX_TEXT)) else { panic!() };
        assert!(text.len() <= MAX_TEXT);
        assert!(text.chars().all(|c| c == 'é'));
    }
}
