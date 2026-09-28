//! A bridge's server, running: started in Grenat's sandbox
//! (`grenat_sandbox`) in the facet's directory, requests written on its
//! standard input, responses read from its standard output by a thread of
//! their own, and its standard error sent to the log, its last lines kept
//! to explain a death.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use grenat_sandbox::Sandboxed;
use serde_json::Value as Json;

use crate::protocol::{self, Reply};

/// Where the lines a server writes on its standard error go: `--log`.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// How many of its last lines of standard error explain a server's death.
const TAIL: usize = 10;

/// What starts a server.
pub struct Launch<'a> {
    pub facet: &'a str,
    pub dir: &'a Path,
    pub argv: &'a [String],
    /// Its environment, besides what the sandbox keeps.
    pub env: Vec<(String, String)>,
    pub network: bool,
    pub log: Option<Log>,
}

/// Why a request got no response.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The server was gone before it could read the request.
    NotDelivered(String),
    /// The server died after the request was sent.
    Exited(String),
    TimedOut,
}

pub struct Process {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<(u64, Reply)>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// Closed when the thread reading standard error is done.
    stderr_done: Receiver<()>,
    next_id: u64,
}

impl Process {
    pub fn start(launch: Launch) -> Result<Process, String> {
        let argv = program_in(launch.dir, launch.argv);
        let sandboxed = Sandboxed { argv: &argv, cwd: Some(launch.dir), env: &launch.env, network: launch.network };
        let mut command = sandboxed.command()?;
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start the process of facet `{}` (`{}`): {e}", launch.facet, argv.join(" ")))?;
        let (stdout, stderr) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
        let (send, responses) = channel();
        let facet = launch.facet.to_string();
        let log = launch.log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match protocol::response(&line) {
                    Some(response) => {
                        if send.send(response).is_err() {
                            break;
                        }
                    }
                    None => {
                        if let Some(log) = &log {
                            log(&format!("[bridge] {facet} (not JSON-RPC): {line}\n"));
                        }
                    }
                }
            }
        });
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let (done, stderr_done) = channel::<()>();
        let (lines, facet, log) = (tail.clone(), launch.facet.to_string(), launch.log);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Some(log) = &log {
                    log(&format!("[bridge] {facet}: {line}\n"));
                }
                let mut lines = lines.lock().unwrap_or_else(|e| e.into_inner());
                if lines.len() == TAIL {
                    lines.pop_front();
                }
                lines.push_back(line);
            }
            drop(done);
        });
        Ok(Process { stdin: child.stdin.take(), child, responses, stderr: tail, stderr_done, next_id: 1 })
    }

    /// Whether the server still runs.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Sends a request, and waits `timeout` at most for its response; a
    /// server that does not answer in time is killed.
    pub fn request(&mut self, method: &str, params: Option<Json>, timeout: Duration) -> Result<Reply, Failure> {
        let id = self.next_id;
        self.next_id += 1;
        let line = protocol::request(id, method, params);
        let sent =
            self.stdin.as_mut().is_some_and(|stdin| writeln!(stdin, "{line}").and_then(|()| stdin.flush()).is_ok());
        if !sent {
            return Err(Failure::NotDelivered(self.death()));
        }
        let deadline = Instant::now() + timeout;
        loop {
            match self.responses.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok((answered, reply)) if answered == id => return Ok(reply),
                // the answer to an older request, given up on
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    self.kill();
                    return Err(Failure::TimedOut);
                }
                Err(RecvTimeoutError::Disconnected) => return Err(Failure::Exited(self.death())),
            }
        }
    }

    /// How the server ended — `exited with status 3`, `was killed by signal
    /// 9` — and its last lines of standard error.
    fn death(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                _ => {
                    self.kill();
                    break None;
                }
            }
        };
        let _ = self.stderr_done.recv_timeout(Duration::from_secs(1));
        let how = match status.map(|s| (s.code(), signal(&s))) {
            Some((Some(code), _)) => format!("exited with status {code}"),
            Some((None, Some(signal))) => format!("was killed by signal {signal}"),
            _ => "stopped answering".to_string(),
        };
        let lines = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
        if lines.is_empty() {
            how
        } else {
            format!("{how}:\n  {}", lines.iter().cloned().collect::<Vec<_>>().join("\n  "))
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Process {
    /// Standard input closed, the server ends by itself (the helper
    /// libraries do); one that does not is killed. Its standard error is
    /// logged to the end.
    fn drop(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_millis(500);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.kill();
        let _ = self.stderr_done.recv_timeout(Duration::from_secs(1));
    }
}

/// A program named by a relative path (`bin/server`) is the facet's own;
/// a bare name (`ruby`) is looked up in `PATH`.
fn program_in(dir: &Path, argv: &[String]) -> Vec<String> {
    let mut argv = argv.to_vec();
    if let Some(program) = argv.first_mut()
        && program.contains('/')
        && Path::new(program.as_str()).is_relative()
    {
        *program = dir.join(PathBuf::from(program.as_str())).to_string_lossy().into_owned();
    }
    argv
}

#[cfg(unix)]
fn signal(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

#[cfg(not(unix))]
fn signal(_: &std::process::ExitStatus) -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, log: Option<Log>) -> Process {
        let argv: Vec<String> = ["sh", "-c", script].map(String::from).to_vec();
        let dir = std::env::temp_dir();
        Process::start(Launch { facet: "t", dir: &dir, argv: &argv, env: Vec::new(), network: true, log }).unwrap()
    }

    #[test]
    fn a_response_is_matched_to_its_request() {
        // answers every request with its id, after a stray line and an older answer
        let script = r#"while read line; do id=$(echo "$line" | sed 's/.*"id":\([0-9]*\).*/\1/'); echo "hello"; echo '{"jsonrpc":"2.0","id":0,"result":0}'; echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":$id}"; done"#;
        let logged = Arc::new(Mutex::new(String::new()));
        let sink = logged.clone();
        let mut process = sh(script, Some(Arc::new(move |line: &str| sink.lock().unwrap().push_str(line))));
        let timeout = Duration::from_secs(5);
        assert_eq!(process.request("describe", None, timeout), Ok(Reply::Result(Json::from(1))));
        assert_eq!(process.request("describe", None, timeout), Ok(Reply::Result(Json::from(2))));
        assert!(process.is_running());
        drop(process);
        assert!(logged.lock().unwrap().contains("[bridge] t (not JSON-RPC): hello\n"), "{}", logged.lock().unwrap());
    }

    #[test]
    fn a_death_is_explained_by_the_last_lines_of_standard_error() {
        let logged = Arc::new(Mutex::new(String::new()));
        let sink = logged.clone();
        let mut process = sh(
            "read line; echo 'on fire' >&2; exit 3",
            Some(Arc::new(move |line: &str| sink.lock().unwrap().push_str(line))),
        );
        let failure = process.request("call", None, Duration::from_secs(5)).unwrap_err();
        assert_eq!(failure, Failure::Exited("exited with status 3:\n  on fire".into()));
        assert!(!process.is_running());
        assert_eq!(*logged.lock().unwrap(), "[bridge] t: on fire\n");
        // gone: the next request is not delivered
        assert!(matches!(process.request("call", None, Duration::from_secs(5)), Err(Failure::NotDelivered(_))));
    }

    #[test]
    fn a_server_that_does_not_answer_in_time_is_killed() {
        let mut process = sh("sleep 30", None);
        let started = Instant::now();
        assert_eq!(process.request("call", None, Duration::from_millis(200)), Err(Failure::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!process.is_running());
    }

    #[test]
    fn a_relative_program_is_the_facets() {
        let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let dir = Path::new("/facet");
        assert_eq!(program_in(dir, &argv(&["bin/server", "-v"])), argv(&["/facet/bin/server", "-v"]));
        assert_eq!(program_in(dir, &argv(&["ruby", "bridge/server.rb"])), argv(&["ruby", "bridge/server.rb"]));
        assert_eq!(program_in(dir, &argv(&["/usr/bin/ruby"])), argv(&["/usr/bin/ruby"]));
        let e = Process::start(Launch {
            facet: "t",
            dir,
            argv: &argv(&["no-such-program-x"]),
            env: Vec::new(),
            network: true,
            log: None,
        })
        .err()
        .unwrap();
        assert!(e.starts_with("cannot start the process of facet `t` (`no-such-program-x`)"), "{e}");
    }
}
