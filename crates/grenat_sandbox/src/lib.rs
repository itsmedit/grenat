//! **Sandboxed processes**: how Grenat starts a program outside itself —
//! `Shell.run`'s programs, the processes of bridge facets.
//!
//! A program is an argument vector (never a shell line), started with a
//! clean environment (only `PATH`, `HOME` and `LANG` are kept, plus the
//! variables given), in a working directory, and — unless it may use the
//! network — inside a sandbox that cuts the network off: `sandbox-exec` on
//! macOS, `unshare` on Linux ([`isolation`]).
//!
//! What is done with the process (its pipes, its timeout) is the caller's.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

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
        if self.argv.is_empty() {
            return Err("no program to run".into());
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

/// The prefix that cuts the network off, if asked: `sandbox-exec` on macOS,
/// `unshare` on Linux. Never runs without isolation when it was asked for:
/// an error says it is not available here.
pub fn isolation(network: bool) -> Result<Vec<String>, String> {
    if network {
        return Ok(Vec::new());
    }
    let found = |program: &str| {
        std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(program).is_file()))
    };
    if cfg!(target_os = "macos") && found("sandbox-exec") {
        return Ok(["sandbox-exec", "-p", "(version 1)(allow default)(deny network*)"].map(String::from).to_vec());
    }
    if cfg!(target_os = "linux") && found("unshare") {
        return Ok(["unshare", "--map-root-user", "--net", "--"].map(String::from).to_vec());
    }
    Err("network isolation is not available here (it needs sandbox-exec or unshare): use `network: true`".into())
}

/// Whether isolation works here, tried once per process: the tool may be
/// installed and still refused (user namespaces disabled, a container's
/// seccomp profile). The error says why it does not.
pub fn isolation_works() -> Result<(), String> {
    static WORKS: OnceLock<Result<(), String>> = OnceLock::new();
    WORKS.get_or_init(probe).clone()
}

fn probe() -> Result<(), String> {
    let argv = ["true".to_string()];
    let mut command = Sandboxed { argv: &argv, cwd: None, env: &[], network: false }
        .command()
        .map_err(|e| e.trim_end_matches(": use `network: true`").to_string())?;
    let out = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("the sandbox cannot start: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let why = String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or_default().trim().to_string();
        Err(format!("the sandbox is refused here ({why})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_network_is_cut_off_by_a_prefix() {
        assert!(isolation(true).unwrap().is_empty());
        match isolation(false) {
            Ok(prefix) if cfg!(target_os = "macos") => assert_eq!(prefix[0], "sandbox-exec"),
            Ok(prefix) => assert_eq!(prefix[..3], ["unshare", "--map-root-user", "--net"]),
            Err(e) => assert!(e.starts_with("network isolation is not available here"), "{e}"),
        }
        // tried once: the answer is the same every time
        let first = isolation_works();
        assert_eq!(first, isolation_works());
        if let Err(e) = first {
            eprintln!("note: network isolation does not work here: {e}");
        }
    }
}
