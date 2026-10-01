//! What wakes a served program: schedules and webhooks.
//!
//! [`Cron`] parses a schedule and computes its next occurrence (UTC);
//! [`signature`] checks that a webhook comes from who claims to send it;
//! [`Server`] receives HTTP requests, answered whole or as a stream of
//! [`events`]. Nothing here knows the language.

pub mod calendar;
mod cron;
pub mod events;
mod server;
pub mod signature;

pub use cron::Cron;
pub use events::EventStream;
pub use server::{Incoming, Server};
