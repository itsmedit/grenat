//! **Bridge facets**, Grenat's side: a facet whose functions are written in
//! Ruby or Python, served by a separate process — no Ruby or Python is
//! embedded in Grenat.
//!
//! The facet names its server in its `grenat.toml` (`[bridge] command =
//! ["ruby", "bridge/server.rb"]`, a [`Spec`]); the server uses the helper
//! library Grenat ships ([`helpers`]: `grenat/bridge` for Ruby,
//! `grenat_bridge` for Python) and speaks JSON-RPC 2.0 on its standard input
//! and output ([`protocol`]):
//!
//! - [`install`]: at `setter install`, the process is asked to `describe`
//!   itself — the same manifest as a native library's — and the `native def`
//!   declarations it stands for are written next to it ([`layout`]);
//! - [`process`]: a server running in Grenat's sandbox (`grenat_sandbox`: a
//!   clean environment, no network unless a function declares `net`), its
//!   standard error sent to the log;
//! - [`bridge`]: one process per facet, started on first use and kept alive,
//!   one call at a time, each within a timeout; a process that died is
//!   started again, and one killed is killed with its process group;
//! - [`registry`]: the bridge functions of a program, by name (what the
//!   interpreter holds next to native libraries).
//!
//! Like native code, a bridge runs outside Grenat's capabilities and taint
//! tracking: which facets may ship one is the application's decision
//! (`facet "texts", bridge: true`), enforced by `grenat_package`.

pub mod bridge;
#[cfg(feature = "fixture")]
pub mod fixture;
mod group;
pub mod helpers;
pub mod install;
pub mod layout;
mod lines;
pub mod process;
pub mod protocol;
pub mod registry;
pub mod spec;

pub use bridge::Bridge;
pub use install::{Installed, install};
pub use process::Log;
pub use registry::Registry;
pub use spec::Spec;
