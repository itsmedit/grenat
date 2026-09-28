//! How a facet's bridge process starts: `[bridge]` in its `grenat.toml`.
//!
//! ```toml
//! [bridge]
//! command = ["ruby", "bridge/server.rb"]   # run in the facet's directory
//! env = ["SHEETS_API_URL"]                 # passed on from Grenat's environment
//! timeout = 30                             # seconds, per call (the default; a day at most)
//! ```

use std::time::Duration;

/// Each call's limit, unless the facet sets its own.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest limit a facet may set: a day.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    /// The server: a program and its arguments, never a shell line.
    pub command: Vec<String>,
    /// The variables of Grenat's environment the process sees, if set:
    /// its environment is clean otherwise.
    pub env: Vec<String>,
    pub timeout: Duration,
}

impl Spec {
    /// A server started by `command`, with the defaults.
    pub fn new(command: &[&str]) -> Spec {
        Spec { command: command.iter().map(|s| s.to_string()).collect(), env: Vec::new(), timeout: DEFAULT_TIMEOUT }
    }

    /// The command as a line, for messages: `ruby bridge/server.rb`.
    pub fn line(&self) -> String {
        self.command.join(" ")
    }

    /// Why `name` cannot be a variable passed on, if it cannot.
    pub fn check_variable(name: &str) -> Result<(), String> {
        let valid = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if valid { Ok(()) } else { Err(format!("`{name}` is not an environment variable's name")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_has_defaults_and_names_variables() {
        let spec = Spec::new(&["ruby", "bridge/server.rb"]);
        assert_eq!((spec.line(), spec.timeout), ("ruby bridge/server.rb".to_string(), DEFAULT_TIMEOUT));
        assert!(Spec::check_variable("SHEETS_URL").is_ok() && Spec::check_variable("_x1").is_ok());
        for bad in ["", "1X", "A-B", "A=B", "A B"] {
            assert!(Spec::check_variable(bad).is_err(), "{bad}");
        }
    }
}
