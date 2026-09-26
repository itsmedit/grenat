//! The Model Context Protocol: a client, and the server side.
//!
//! An MCP server offers tools (name, description, JSON Schema of their
//! input, hints such as "read-only" or "destructive"); a [`Client`]
//! connects to one — a program spoken to over its standard input and output,
//! or a URL (streamable HTTP) — lists its tools and calls them; [`server`]
//! answers such a client for a [`server::Host`]. JSON-RPC 2.0 throughout;
//! nothing here knows about the Grenat language.

mod client;
#[cfg(feature = "fake")]
pub mod fake;
pub mod server;
mod transport;

pub use client::{CallResult, Client, Tool};
pub use transport::{Http, Stdio, Transport};

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
