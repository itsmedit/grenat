//! What wakes a served program: schedules and webhooks.
//!
//! [`Cron`] parses a schedule and computes its next occurrence (UTC);
//! [`signature`] checks that a webhook comes from who claims to send it
//! (GitHub, Slack, Microsoft Teams, a bearer token), and [`replay`] that a
//! signed request is accepted once;
//! [`Server`] receives HTTP/1.x requests (a small server of its own:
//! [`request`] read within limits, a thread per connection, every client
//! timed), answered whole ([`response`]) or as a stream of [`events`].
//! Nothing here knows the language.

pub mod calendar;
mod connection;
mod cron;
mod descriptors;
pub mod events;
mod outbox;
pub mod replay;
mod request;
mod response;
mod server;
pub mod signature;
mod status;

pub use connection::Timeouts;
pub use cron::Cron;
pub use events::EventStream;
pub use request::Sizes;
pub use server::{Incoming, Server, Settings};
