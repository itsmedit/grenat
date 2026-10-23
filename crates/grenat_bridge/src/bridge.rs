//! A facet's bridge: one server process, started on first use and kept
//! alive for the next calls.
//!
//! - **One call at a time.** Calls are serialized (a lock around the
//!   process): the helper libraries serve a request after the other, and a
//!   server never sees two at once. Concurrent Grenat tasks wait their turn.
//! - **A timeout per call** (the facet's `timeout`): a server that does not
//!   answer in time is killed, and the call raises; the next one starts a
//!   new server.
//! - **Restarts.** A call that finds its server dead (or dying before it
//!   could read the request) starts it again, once. A server that dies
//!   while it handles a call is started again and the call sent anew only
//!   if the function is `pure` — a call with effects is never run twice —
//!   else the call raises, with the last lines of its standard error.
//! - **The network** is cut off (`grenat_sandbox`) unless one of the facet's
//!   functions declares `net`. Where the sandbox is missing or refused (no
//!   `sandbox-exec`, user namespaces disabled, a container), the server is
//!   not started — there is no running it unsandboxed — and the call
//!   raises: which sandbox, on which system, and what to do.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use grenat_native::{Manifest, Outcome};
use grenat_sandbox::Probe;
use serde_json::Value as Json;

use crate::helpers;
use crate::layout::Layout;
use crate::process::{Failure, Launch, Log, Process};
use crate::protocol::{self, Reply};
use crate::spec::Spec;

pub struct Bridge {
    pub facet: String,
    pub dir: PathBuf,
    pub spec: Spec,
    /// Whether the server may use the network.
    pub network: bool,
    log: Option<Log>,
    /// Whether the sandbox works here (`grenat_sandbox::isolation_works`).
    probe: Probe,
    process: Mutex<Option<Process>>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("facet", &self.facet)
            .field("command", &self.spec.command)
            .finish_non_exhaustive()
    }
}

/// Whether the functions of `manifest` may use the network: one of them declares `net`.
pub fn needs_network(manifest: &Manifest) -> bool {
    let net = |effect: &str| effect.split(['.', '(']).next() == Some("net");
    manifest.functions.iter().any(|f| f.effects.iter().any(|e| net(e)))
}

impl Bridge {
    /// The bridge of the facet `facet`, installed in `dir`; not started yet.
    pub fn new(facet: &str, dir: &Path, spec: &Spec, network: bool, log: Option<Log>) -> Bridge {
        Bridge {
            facet: facet.to_string(),
            dir: dir.to_path_buf(),
            spec: spec.clone(),
            network,
            log,
            probe: grenat_sandbox::isolation_works,
            process: Mutex::new(None),
        }
    }

    /// The same bridge, `probe` telling whether the sandbox works here (a
    /// stand-in, for tests).
    pub fn probed(self, probe: Probe) -> Bridge {
        Bridge { probe, ..self }
    }

    /// The server's manifest, as it describes itself.
    pub fn describe(&self) -> Result<Json, String> {
        match self.request("describe", None, true, "describe")? {
            Reply::Result(manifest) => Ok(manifest),
            Reply::Error(e) => {
                Err(format!("the process of facet `{}` cannot describe itself: {}", self.facet, e.message))
            }
        }
    }

    /// Calls `name` with `args` (a JSON array); `pure`: it may be sent again
    /// to a new server if the first one dies handling it.
    pub fn call(&self, name: &str, args: &[u8], pure: bool) -> Result<Outcome, String> {
        let args: Json = serde_json::from_slice(args).map_err(|e| format!("the arguments of `{name}`: {e}"))?;
        Ok(match self.request("call", Some(protocol::call(name, args)), pure, &format!("`{name}`"))? {
            Reply::Result(result) => Outcome::Returned(result),
            Reply::Error(e) => Outcome::Raised { ty: e.grenat_type(), message: e.message },
        })
    }

    /// Sends a request to the server, started (again) if it is not running.
    fn request(&self, method: &str, params: Option<Json>, replay: bool, what: &str) -> Result<Reply, String> {
        let mut process = self.process.lock().unwrap_or_else(|e| e.into_inner());
        let mut restarted = false;
        loop {
            if !process.as_mut().is_some_and(Process::is_running) {
                *process = None;
                *process = Some(self.start()?);
            }
            let running = process.as_mut().expect("started");
            let failure = match running.request(method, params.clone(), self.spec.timeout) {
                Ok(reply) => return Ok(reply),
                Err(failure) => failure,
            };
            *process = None;
            let facet = &self.facet;
            let message = match &failure {
                Failure::TimedOut => {
                    return Err(format!(
                        "{what} still running after {:?}: the process of facet `{facet}` was killed",
                        self.spec.timeout
                    ));
                }
                Failure::NotDelivered(death) => format!("the process of facet `{facet}` {death}"),
                Failure::Exited(death) => format!("the process of facet `{facet}` {death} (during {what})"),
            };
            let again = replay || matches!(failure, Failure::NotDelivered(_));
            if restarted || !again {
                return Err(message);
            }
            restarted = true;
        }
    }

    fn start(&self) -> Result<Process, String> {
        let lib = Layout::new(&self.dir).lib();
        helpers::write(&lib)?;
        let mut env = helpers::env(&lib);
        for name in &self.spec.env {
            if let Ok(value) = std::env::var(name) {
                env.push((name.clone(), value));
            }
        }
        if !self.network
            && let Err(why) = (self.probe)()
        {
            return Err(format!(
                "the process of facet `{}` is not started: {why} (a bridge facet never runs without the sandbox)",
                self.facet
            ));
        }
        Process::start(Launch {
            facet: &self.facet,
            dir: &self.dir,
            argv: &self.spec.command,
            env,
            network: self.network,
            log: self.log.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grenat_native::Function;

    fn function(effects: &[&str]) -> Function {
        Function {
            name: "f".into(),
            symbol: "f".into(),
            doc: None,
            params: Vec::new(),
            returns: "Nil".into(),
            effects: effects.iter().map(|e| e.to_string()).collect(),
            pure: false,
            error: "BridgeError".into(),
        }
    }

    #[test]
    fn only_a_function_declaring_net_opens_the_network() {
        let manifest = |effects: &[&[&str]]| Manifest {
            abi: 1,
            functions: effects.iter().map(|e| function(e)).collect(),
            structs: Vec::new(),
        };
        assert!(!needs_network(&manifest(&[&[], &["fs.read", "time"]])));
        assert!(!needs_network(&manifest(&[&["network_card"]])));
        for net in ["net", "net(\"api.x.com\")", "net.http"] {
            assert!(needs_network(&manifest(&[&["fs.read"], &[net]])), "{net}");
        }
    }

    fn unit_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("grenat-bridge-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A bridge served by `command`, in a directory of its own.
    fn bridge(command: &[&str]) -> Bridge {
        Bridge::new("t", &unit_dir(), &Spec::new(command), true, None)
    }

    fn no_sandbox() -> Result<(), String> {
        Err(grenat_sandbox::Missing::NotFound.message())
    }

    #[test]
    fn a_server_is_never_started_without_its_sandbox() {
        let dir = unit_dir().join("unsandboxed");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let started = dir.join("started");
        let script = format!("touch {}; exit 3", started.display());
        let spec = Spec::new(&["sh", "-c", &script]);
        let cut = Bridge::new("t", &dir, &spec, false, None).probed(no_sandbox);
        for e in [cut.call("f", b"[]", true).unwrap_err(), cut.describe().unwrap_err()] {
            assert!(
                e.starts_with("the process of facet `t` is not started: network isolation is not available here (")
            );
            assert!(e.contains("is not found on PATH") || e.contains("only"), "{e}");
            assert!(e.ends_with("(a bridge facet never runs without the sandbox)"), "{e}");
        }
        assert!(!started.exists(), "the server ran unsandboxed");
        // a facet that declares `net` has nothing cut off: it needs no sandbox
        let open = Bridge::new("t", &dir, &spec, true, None).probed(no_sandbox);
        assert!(open.call("f", b"[]", true).unwrap_err().contains("exited with status 3"));
        assert!(started.exists());
    }

    #[test]
    fn a_server_that_cannot_start_fails_the_call() {
        let e = bridge(&["no-such-program-x"]).call("f", b"[]", true).unwrap_err();
        assert!(e.starts_with("cannot start the process of facet `t` (`no-such-program-x`)"), "{e}");
        // a server that dies at once: started again, once, for a call it never read
        let bridge = bridge(&["sh", "-c", "echo 'no server here' >&2; exit 2"]);
        let e = bridge.call("f", b"[]", false).unwrap_err();
        assert!(e.starts_with("the process of facet `t` exited with status 2"), "{e}");
        assert!(e.contains("no server here"), "{e}");
        assert!(bridge.call("f", b"not json", false).unwrap_err().starts_with("the arguments of `f`"));
    }
}
