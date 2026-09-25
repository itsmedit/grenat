//! What the environment asks for.

use std::env;
use std::io::IsTerminal;

pub fn use_color() -> bool {
    std::io::stderr().is_terminal() && env::var_os("NO_COLOR").is_none()
}

/// `GRENAT_LOG`: log LLM and tool calls, and what runs natively.
pub fn log_from_env() -> bool {
    env::var_os("GRENAT_LOG").is_some_and(|v| v != "0")
}

/// `GRENAT_JIT=0`: interpret everything.
pub fn native_from_env() -> bool {
    env::var_os("GRENAT_JIT").is_none_or(|v| v != "0")
}

/// `GRENAT_RECORD=1`: record every cassette again, with real calls.
pub fn record_from_env() -> bool {
    env::var_os("GRENAT_RECORD").is_some_and(|v| v != "0")
}
