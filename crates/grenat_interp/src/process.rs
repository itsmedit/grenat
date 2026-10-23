//! Running a program: an argument vector (never a shell line), a clean
//! environment, a working directory and optionally no network (all
//! `grenat_sandbox`'s), its outputs read whole, within a timeout.
//!
//! `network: false` where the sandbox is missing is refused, as a bridge
//! facet's server is: the program is not started, and the error says
//! which sandbox, on which system, and what to do.

use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

use grenat_sandbox::{Probe, Sandboxed, UNAVAILABLE};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProcessRequest {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    /// Added to a clean environment (`PATH`, `HOME` and `LANG` are kept).
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    pub network: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProcessReply {
    pub status: i64,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `request`; an error is a program that could not run or ran too long.
pub(crate) fn run(request: &ProcessRequest) -> Result<ProcessReply, String> {
    run_probed(request, grenat_sandbox::isolation_works)
}

/// [`run`], `probe` telling whether the sandbox works here.
fn run_probed(request: &ProcessRequest, probe: Probe) -> Result<ProcessReply, String> {
    let cwd = request.cwd.as_deref().map(std::path::Path::new);
    let sandboxed = Sandboxed { argv: &request.argv, cwd, env: &request.env, network: request.network };
    let mut command = sandboxed.command_probed(probe).map_err(|e| refused(&request.argv, e))?;
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("cannot run `{}`: {e}", request.argv[0]))?;
    let read = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut stream) = stream {
                let mut bytes = Vec::new();
                let _ = stream.read_to_end(&mut bytes);
                text = String::from_utf8_lossy(&bytes).into_owned();
            }
            text
        })
    };
    let stdout = read(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let stderr = read(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let deadline = Instant::now() + request.timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("`{}` still running after {:?}: killed", request.argv[0], request.timeout));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    Ok(ProcessReply {
        status: status.code().map_or(-1, i64::from),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// A program the sandbox refused: which one, why, and the other way.
fn refused(argv: &[String], why: String) -> String {
    match argv.first() {
        Some(program) if why.starts_with(UNAVAILABLE) => {
            format!("`{program}` is not started: {why} (or run it with `network: true`)")
        }
        _ => why,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing() -> Result<(), String> {
        Err(grenat_sandbox::Missing::Refused("no namespaces".into()).message())
    }

    fn request(network: bool) -> ProcessRequest {
        let argv = ["sh", "-c", "echo started"].map(String::from).to_vec();
        ProcessRequest { argv, cwd: None, env: Vec::new(), timeout: Duration::from_secs(10), network }
    }

    #[test]
    fn network_false_without_a_sandbox_is_refused() {
        let e = run_probed(&request(false), missing).unwrap_err();
        assert!(e.starts_with("`sh` is not started: network isolation is not available here ("), "{e}");
        assert!(e.contains("refuses to run: no namespaces"), "{e}");
        assert!(e.ends_with("(or run it with `network: true`)"), "{e}");
        // with the network, no sandbox is needed
        assert_eq!(run_probed(&request(true), missing).unwrap().stdout, "started\n");
    }
}
