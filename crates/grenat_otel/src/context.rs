//! A span's context, and the W3C `traceparent` header that carries it
//! between processes (<https://www.w3.org/TR/trace-context/>):
//! `00-<trace id>-<parent id>-<flags>`.

use crate::ids::{SpanId, TraceId};

/// What a child needs of its parent: the trace, and the parent's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanContext {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    /// Whether the caller samples the trace (bit 0 of the flags).
    pub sampled: bool,
}

impl SpanContext {
    /// The `traceparent` header naming this span as the parent.
    pub fn traceparent(&self) -> String {
        format!("00-{}-{}-{:02x}", self.trace_id.hex(), self.span_id.hex(), u8::from(self.sampled))
    }

    /// A `traceparent` header received: `None` when it is invalid, which
    /// starts a new trace. A version above `00` is read as `00` (its
    /// first four fields), as the specification asks; `ff` is invalid.
    pub fn from_traceparent(header: &str) -> Option<SpanContext> {
        let header = header.trim();
        let mut fields = header.split('-');
        let version = fields.next()?;
        let (trace, parent, flags) = (fields.next()?, fields.next()?, fields.next()?);
        if version.len() != 2 || version == "ff" || !version.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        if version == "00" && (fields.next().is_some() || header.len() != 55) {
            return None;
        }
        if flags.len() != 2 {
            return None;
        }
        let flags = u8::from_str_radix(flags, 16).ok()?;
        Some(SpanContext { trace_id: TraceId::parse(trace)?, span_id: SpanId::parse(parent)?, sampled: flags & 1 == 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn the_specifications_example_round_trips() {
        let context = SpanContext::from_traceparent(EXAMPLE).unwrap();
        assert_eq!(context.trace_id.hex(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(context.span_id.hex(), "00f067aa0ba902b7");
        assert!(context.sampled);
        assert_eq!(context.traceparent(), EXAMPLE);
    }

    #[test]
    fn invalid_headers_start_a_new_trace() {
        for bad in [
            "",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-1",
        ] {
            assert_eq!(SpanContext::from_traceparent(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_later_version_is_read_as_version_00() {
        let later = "cc-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00-what-comes-next";
        let context = SpanContext::from_traceparent(later).unwrap();
        assert!(!context.sampled);
        assert_eq!(context.span_id.hex(), "00f067aa0ba902b7");
    }
}
