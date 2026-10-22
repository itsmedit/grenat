//! **Sandboxed processes**: how Grenat starts a program outside itself —
//! `Shell.run`'s programs, the processes of bridge facets.
//!
//! A program is an argument vector (never a shell line), started with a
//! clean environment (only `PATH`, `HOME` and `LANG` are kept, plus the
//! variables given), in a working directory, and — unless it may use the
//! network — inside a sandbox that cuts the network off: `sandbox-exec` on
//! macOS, `unshare` on Linux ([`isolation`]). Where that sandbox is
//! missing or refused, the program is not started: the error says which
//! sandbox, on which system, and what to do.
//!
//! What is done with the process (its pipes, its timeout) is the caller's.

mod isolation;

use std::path::Path;
use std::process::Command;

pub use isolation::{Missing, Probe, UNAVAILABLE, isolation, isolation_works};

/// The variables of Grenat's own environment a program keeps.
pub const KEPT: [&str; 3] = ["PATH", "HOME", "LANG"];

/// What to start.
#[derive(Debug, Clone, Copy)]
pub struct Sandboxed<'a> {
    pub argv: &'a [String],
    pub cwd: Option<&'a Path>,
    /// Added to the clean environment.
    pub env: &'a [(String, String)],
    /// `false`: the network is cut off, or the program is not started.
    pub network: bool,
}

impl Sandboxed<'_> {
    /// The command that starts the program: its pipes are left to the caller.
    pub fn command(&self) -> Result<Command, String> {
        self.command_probed(isolation_works)
    }

    /// [`Sandboxed::command`], `probe` telling whether isolation works here
    /// (asked only when the network is to be cut off).
    pub fn command_probed(&self, probe: Probe) -> Result<Command, String> {
        if self.argv.is_empty() {
            return Err("no program to run".into());
        }
        if !self.network {
            probe()?;
        }
        let mut argv = isolation(self.network)?;
        argv.extend(self.argv.iter().cloned());
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]).env_clear();
        for kept in KEPT {
            if let Some(value) = std::env::var_os(kept) {
                command.env(kept, value);
            }
        }
        command.envs(self.env.iter().map(|(k, v)| (k, v)));
        if let Some(dir) = self.cwd {
            command.current_dir(dir);
        }
        Ok(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn output(sandboxed: Sandboxed) -> String {
        let out = sandboxed.command().unwrap().stdin(Stdio::null()).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn a_program_gets_a_clean_environment_and_a_directory() {
        let argv: Vec<String> = ["sh", "-c", "echo x${CARGO_PKG_NAME}x $GIVEN $(pwd)"].map(String::from).to_vec();
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let env = [("GIVEN".to_string(), "yes".to_string())];
        let out = output(Sandboxed { argv: &argv, cwd: Some(&dir), env: &env, network: true });
        // the parent's environment is not passed on, only what is given and kept
        assert_eq!(out, format!("xx yes {}\n", dir.display()));
        let argv: Vec<String> = ["sh", "-c", "echo $PATH"].map(String::from).to_vec();
        assert!(!output(Sandboxed { argv: &argv, cwd: None, env: &[], network: true }).trim().is_empty());
    }

    #[test]
    fn no_program_is_an_error() {
        let e = Sandboxed { argv: &[], cwd: None, env: &[], network: true }.command().unwrap_err();
        assert_eq!(e, "no program to run");
    }

    #[test]
    fn no_program_starts_without_the_sandbox_it_asked_for() {
        fn missing() -> Result<(), String> {
            Err(Missing::NotFound.message())
        }
        let argv = ["true".to_string()];
        let cut = Sandboxed { argv: &argv, cwd: None, env: &[], network: false };
        let e = cut.command_probed(missing).unwrap_err();
        assert!(e.starts_with(UNAVAILABLE), "{e}");
        // with the network, there is nothing to cut off: the sandbox is not asked
        assert!(Sandboxed { network: true, ..cut }.command_probed(missing).is_ok());
    }
}
