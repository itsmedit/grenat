//! Running a program: an argument vector (never a shell line), a clean
//! environment, a working directory, a timeout, and optionally no network.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
    let mut argv = isolation(request.network)?;
    argv.extend(request.argv.iter().cloned());
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for kept in ["PATH", "HOME", "LANG"] {
        if let Some(value) = std::env::var_os(kept) {
            command.env(kept, value);
        }
    }
    command.envs(request.env.iter().map(|(k, v)| (k, v)));
    if let Some(dir) = &request.cwd {
        command.current_dir(dir);
    }
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

/// The prefix that cuts the network off, if asked: `sandbox-exec` on macOS,
/// `unshare` on Linux. Never runs without isolation when it was asked for.
fn isolation(network: bool) -> Result<Vec<String>, String> {
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
