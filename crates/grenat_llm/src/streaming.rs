//! Streamed answers: what a provider gives as the model writes ([`Delta`]s,
//! to a [`Sink`]), and how a stream of events becomes them and then the
//! whole [`Response`] — the same one a call without streaming returns.
//!
//! Each wire format has its [`Decoder`]; [`decode`] reads the events and
//! knows whether a piece already reached the consumer: a failure after that
//! is never retried (the consumer would get the start of the answer twice).

use std::io::Read;
use std::ops::ControlFlow;

use crate::sse::{Event, Events};
use crate::*;

/// A piece of an answer, as it arrives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Delta<'a> {
    /// Text the model writes.
    Text(&'a str),
    /// A piece of the JSON input of a tool call (the call `id`, to the tool
    /// `name`): the pieces of a call, put together, are its input.
    ToolInput { id: &'a str, name: &'a str, json: &'a str },
}

/// Receives the pieces of an answer; `Break` stops the stream (the call then
/// fails with [`STOPPED`]).
pub type Sink<'s> = dyn FnMut(Delta<'_>) -> ControlFlow<()> + 's;

/// The error of a stream its consumer stopped.
pub const STOPPED: &str = "the stream was stopped";

/// A provider that does not stream: its answer, whole, as one piece of
/// text and one piece for the input of each tool call.
pub fn replay(response: &Response, sink: &mut Sink) -> Result<(), LlmError> {
    let text = response.text();
    let stopped = |flow: ControlFlow<()>| if flow.is_break() { Err(LlmError::new(STOPPED)) } else { Ok(()) };
    if !text.is_empty() {
        stopped(sink(Delta::Text(&text)))?;
    }
    for call in response.tool_uses() {
        stopped(sink(Delta::ToolInput { id: &call.id, name: &call.name, json: &call.input.to_string() }))?;
    }
    Ok(())
}

/// Why an attempt failed, and whether another may be made.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Failed {
    pub message: String,
    /// The provider says to try again (overloaded, rate limited…).
    pub retry: bool,
    /// Seconds to wait first, when it says how long.
    pub retry_after: Option<u64>,
    /// What the provider bills for the attempt, when the stream said it
    /// before failing (its usage so far).
    pub billed: Option<Box<Response>>,
}

impl Failed {
    pub(crate) fn fatal(message: impl Into<String>) -> Failed {
        Failed { message: message.into(), retry: false, retry_after: None, billed: None }
    }

    pub(crate) fn retry(message: impl Into<String>) -> Failed {
        Failed { message: message.into(), retry: true, retry_after: None, billed: None }
    }

    /// An HTTP error before the stream: retried as calls without streaming are.
    pub(crate) fn status(status: u16, retry_after: Option<u64>, body: &str) -> Failed {
        let json = serde_json::from_str(body).unwrap_or_else(|_| serde_json::Value::String(body.to_string()));
        Failed {
            message: format!("HTTP {status}: {}", crate::retry::error_message(&json)),
            retry: crate::retry::retryable(status),
            retry_after,
            billed: None,
        }
    }
}

/// What a wire format's events mean.
pub(crate) trait Decoder {
    /// Takes an event, gives its pieces to `out`; the response once the
    /// event that ends the stream has come.
    fn event(&mut self, event: &Event, out: &mut Out) -> Result<Option<Response>, Failed>;

    /// The stream ended without that event: an answer, or why there is none.
    fn ended(&mut self) -> Result<Response, Failed> {
        Err(Failed::retry("the stream ended before the answer was complete"))
    }

    /// What the provider bills so far, when its events said it (a stream
    /// stopped or broken midway is billed too).
    fn billed(&self) -> Option<Response> {
        None
    }
}

/// Where a decoder sends pieces: the consumer, told apart from whether it
/// got any.
pub(crate) struct Out<'a, 's> {
    sink: &'a mut Sink<'s>,
    delivered: bool,
}

impl Out<'_, '_> {
    pub(crate) fn send(&mut self, delta: Delta) -> Result<(), Failed> {
        let empty = match delta {
            Delta::Text(text) => text.is_empty(),
            Delta::ToolInput { json, .. } => json.is_empty(),
        };
        if empty {
            return Ok(());
        }
        self.delivered = true;
        match (self.sink)(delta) {
            ControlFlow::Continue(()) => Ok(()),
            ControlFlow::Break(()) => Err(Failed::fatal(STOPPED)),
        }
    }
}

/// Reads a stream to its end through `decoder`.
pub(crate) fn decode(body: impl Read, decoder: &mut impl Decoder, sink: &mut Sink) -> Result<Response, Failed> {
    let mut events = Events::new(body);
    let mut out = Out { sink, delivered: false };
    let result = (|| {
        while let Some(event) = events.next_event().map_err(Failed::retry)? {
            if let Some(response) = decoder.event(&event, &mut out)? {
                return Ok(response);
            }
        }
        decoder.ended()
    })();
    // part of the answer was given: trying again would give it twice
    result.map_err(|failed| {
        let billed = failed.billed.or_else(|| decoder.billed().map(Box::new));
        Failed { retry: failed.retry && !out.delivered, billed, ..failed }
    })
}

/// The JSON of an event's data.
pub(crate) fn json(event: &Event) -> Result<serde_json::Value, Failed> {
    serde_json::from_str(&event.data)
        .map_err(|e| Failed::fatal(format!("unreadable stream event `{}`: {e}", event.name)))
}
