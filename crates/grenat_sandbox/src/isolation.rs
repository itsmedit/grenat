//! **Network isolation**: the prefix that cuts a program's network off —
//! `sandbox-exec` on macOS, `unshare` on Linux — and whether it works on
//! this machine, tried once per process.
//!
//! Isolation is never skipped when it was asked for: where it is missing,
//! the error names the sandbox, the operating system, and what to do
//! ([`Missing`]). Who asked decides what follows — `Shell.run` refuses
//! `network: false`, a bridge facet's server is not started.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// How a caller learns whether isolation works here: [`isolation_works`],
/// or a stand-in for tests.
pub type Probe = fn() -> Result<(), String>;

/// What every "no sandbox here" error starts with.
pub const UNAVAILABLE: &str = "network isolation is not available here";

/// Why isolation is not available on this machine.
#[derive(Debug, Clone, PartialEq)]
pub enum Missing {
    /// The sandbox's program is not on `PATH`.
    NotFound,
    /// It is there, and refuses to run (its first line of standard error).
    Refused(String),
    /// Grenat has no sandbox for this operating system.
    Unsupported,
}

impl Missing {
    /// The error: which sandbox, on which system, and what to do.
    pub fn message(&self) -> String {
        let (os, tool) = (os_name(), tool());
        match (self, tool) {
            (Missing::Unsupported, _) | (_, None) => format!(
                "{UNAVAILABLE} ({os}: Grenat cuts the network off with `sandbox-exec` on macOS and `unshare` on \
                 Linux only): run it on macOS or Linux"
            ),
            (Missing::NotFound, Some(tool)) => {
                format!("{UNAVAILABLE} ({os}: `{tool}` is not found on PATH): {}", fix(&Missing::NotFound))
            }
            (Missing::Refused(why), Some(tool)) => {
                let why = if why.is_empty() { String::new() } else { format!(": {why}") };
                format!("{UNAVAILABLE} ({os}: `{tool}` refuses to run{why}): {}", fix(self))
            }
        }
    }
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

/// The sandbox's program on this system, if Grenat has one.
fn tool() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("sandbox-exec")
    } else if cfg!(target_os = "linux") {
        Some("unshare")
    } else {
        None
    }
}

fn fix(missing: &Missing) -> &'static str {
    match (missing, cfg!(target_os = "macos")) {
        (Missing::NotFound, true) => "it ships with macOS in /usr/bin: put /usr/bin on PATH",
        (Missing::NotFound, false) => "install util-linux, which provides it",
        (_, true) => "run Grenat outside another sandbox (`sandbox-exec` does not nest)",
        (_, false) => {
            "allow unprivileged user namespaces (sysctl kernel.unprivileged_userns_clone=1, and \
             kernel.apparmor_restrict_unprivileged_userns=0 where AppArmor restricts them), or give the container \
             a seccomp profile that permits `unshare`"
        }
    }
}

/// The prefix that cuts the network off, if asked. It is not tried: a
/// sandbox found and refused shows when the program starts (see
/// [`isolation_works`]).
pub fn isolation(network: bool) -> Result<Vec<String>, String> {
    if network {
        return Ok(Vec::new());
    }
    let Some(tool) = tool() else { return Err(Missing::Unsupported.message()) };
    let found =
        std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(tool).is_file()));
    if !found {
        return Err(Missing::NotFound.message());
    }
    let prefix: &[&str] = if cfg!(target_os = "macos") {
        &["sandbox-exec", "-p", "(version 1)(allow default)(deny network*)"]
    } else {
        &["unshare", "--map-root-user", "--net", "--"]
    };
    Ok(prefix.iter().map(|s| s.to_string()).collect())
}

/// Whether isolation works here, tried once per process: the tool may be
/// installed and still refused (user namespaces disabled, a container's
/// seccomp profile). The error says why it does not, and what to do.
pub fn isolation_works() -> Result<(), String> {
    static WORKS: OnceLock<Result<(), String>> = OnceLock::new();
    WORKS.get_or_init(probe).clone()
}

fn probe() -> Result<(), String> {
    let mut argv = isolation(false)?;
    argv.push("true".into());
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| Missing::Refused(e.to_string()).message())?;
    if out.status.success() {
        Ok(())
    } else {
        let why = String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or_default().trim().to_string();
        Err(Missing::Refused(why).message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_network_is_cut_off_by_a_prefix() {
        assert!(isolation(true).unwrap().is_empty());
        match isolation(false) {
            Ok(prefix) if cfg!(target_os = "macos") => assert_eq!(prefix[0], "sandbox-exec"),
            Ok(prefix) => assert_eq!(prefix[..3], ["unshare", "--map-root-user", "--net"]),
            Err(e) => assert!(e.starts_with(UNAVAILABLE), "{e}"),
        }
        // tried once: the answer is the same every time
        let first = isolation_works();
        assert_eq!(first, isolation_works());
        if let Err(e) = first {
            eprintln!("note: network isolation does not work here: {e}");
        }
    }

    #[test]
    fn a_missing_sandbox_names_itself_its_system_and_the_fix() {
        let not_found = Missing::NotFound.message();
        let refused = Missing::Refused("unshare: write failed /proc/self/uid_map".into()).message();
        let unsupported = Missing::Unsupported.message();
        for e in [&not_found, &refused, &unsupported] {
            assert!(e.starts_with(UNAVAILABLE), "{e}");
        }
        assert!(unsupported.contains("`sandbox-exec` on macOS and `unshare` on Linux only"), "{unsupported}");
        if cfg!(target_os = "macos") {
            assert!(not_found.contains("(macOS: `sandbox-exec` is not found on PATH): it ships with macOS"));
            assert!(refused.contains("(macOS: `sandbox-exec` refuses to run: unshare: write failed"), "{refused}");
            assert!(refused.ends_with("(`sandbox-exec` does not nest)"), "{refused}");
        } else if cfg!(target_os = "linux") {
            assert!(not_found.contains("(Linux: `unshare` is not found on PATH): install util-linux"));
            assert!(refused.contains("(Linux: `unshare` refuses to run: unshare: write failed"), "{refused}");
            assert!(refused.contains("allow unprivileged user namespaces"), "{refused}");
        }
        assert!(Missing::Refused(String::new()).message().contains("refuses to run): ") || tool().is_none());
    }
}
