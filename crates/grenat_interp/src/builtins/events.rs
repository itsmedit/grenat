//! Responses streamed as Server-Sent Events: a route answers before its
//! answer is whole, as a chat shows a model's answer while it is written.
//!
//! ```ruby
//! get "/chat" do |req|
//!   stream do |out|
//!     reply(req.params["q"]) { |chunk| out << chunk }   # each piece, sent at once
//!     out.event("done", "")                             # a named event
//!   end
//! end
//! ```
//!
//! `stream` builds the response; its block runs once the head is sent,
//! `out` writing events to the client as they come (`grenat serve`), or to
//! a list a test reads (`request`). An event's data is text — a string as
//! it is, anything else as JSON — whose lines each become a `data:` line:
//! an untrusted chunk cannot forge an event. When the client goes away,
//! writing raises `StreamClosed`, which stops a streamed model call too.

use std::io;

use crate::prelude::*;

use super::*;

/// What `out` is in `stream do |out| … end`.
pub(crate) const EVENT_STREAM: &str = "EventStream";

/// Where events go.
pub(crate) trait EventSink: Send {
    fn send(&mut self, name: Option<&str>, data: &str) -> io::Result<()>;
}

impl EventSink for grenat_serve::EventStream {
    fn send(&mut self, name: Option<&str>, data: &str) -> io::Result<()> {
        grenat_serve::EventStream::send(self, name, data)
    }
}

/// The events of a stream a test reads: (name, data).
pub(crate) type Collected = Arc<Mutex<Vec<(Option<String>, String)>>>;

impl EventSink for Collected {
    fn send(&mut self, name: Option<&str>, data: &str) -> io::Result<()> {
        self.borrow_mut().push((name.map(String::from), data.to_string()));
        Ok(())
    }
}

/// The `out` of a stream.
pub struct Events {
    sink: Mutex<Box<dyn EventSink>>,
}

/// `stream do |out| … end`: a response whose body the block writes.
pub(crate) fn stream_response<'p>(args: &Args<'p>) -> R<'p> {
    let body = block(args, "stream")?;
    Ok(Value::record(
        RESPONSE_RECORD,
        vec![
            ("status".into(), Value::Int(200)),
            ("content_type".into(), Value::str("text/event-stream")),
            ("headers".into(), Value::Hash(Arc::new(Mutex::new(Vec::new())))),
            ("body".into(), Value::str("")),
            ("stream".into(), body),
        ],
    ))
}

/// `out.event(name, data)`.
pub(crate) fn events_method<'p>(
    interp: &mut Interp<'p>,
    events: &Events,
    name: &str,
    args: &Args<'p>,
) -> Option<R<'p>> {
    match name {
        "event" => Some((|| {
            let event = arg(args, 0, name)?;
            if event.contains_taint() {
                return raise("TaintError", "an untrusted value names an event: name it yourself");
            }
            let event = event.to_display();
            if !grenat_serve::events::valid_name(&event) {
                return raise("ArgumentError", format!("an event's name is one line, not empty: {event:?}"));
            }
            interp.send_event(events, Some(&event), &arg(args, 1, name)?)?;
            Ok(Value::Nil)
        })()),
        _ => None,
    }
}

impl<'p> Interp<'p> {
    /// Runs a stream's block, its events going to `sink`.
    pub(crate) fn run_events(&mut self, block: &Value<'p>, sink: Box<dyn EventSink>) -> Result<(), Ctrl<'p>> {
        let out = Value::Events(Arc::new(Events { sink: Mutex::new(sink) }));
        self.call_block(block, vec![out]).map(drop)
    }

    /// Writes one event: a string as it is, any other value as JSON.
    pub(crate) fn send_event(&mut self, events: &Events, name: Option<&str>, data: &Value<'p>) -> Result<(), Ctrl<'p>> {
        if data.contains_secret() {
            return raise("SecretError", "a secret is never sent to a client");
        }
        let text = match data.untainted() {
            Value::Str(text) => text.to_string(),
            other => crate::llm::value_to_json(other).to_string(),
        };
        let sent = events.sink.borrow_mut().send(name, &text);
        sent.or_else(|_| raise("StreamClosed", "the client closed the stream"))
    }
}
