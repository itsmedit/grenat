//! Running a program: an argument vector (never a shell line), a clean
//! environment, a working directory and optionally no network (all
//! `grenat_sandbox`'s), its outputs read whole, within a timeout.

use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

use grenat_sandbox::Sandboxed;

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
    let cwd = request.cwd.as_deref().map(std::path::Path::new);
    let sandboxed = Sandboxed { argv: &request.argv, cwd, env: &request.env, network: request.network };
    let mut command = sandboxed.command()?;
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
