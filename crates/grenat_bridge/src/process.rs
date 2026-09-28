//! A bridge's server, running: started in Grenat's sandbox
//! (`grenat_sandbox`) in the facet's directory and in a process group of
//! its own ([`crate::group`]), requests written on its standard input by a
//! thread of their own, responses read from its standard output by
//! another, and its standard error sent to the log, its last lines kept to
//! explain a death.
//!
//! A request's timeout covers writing it as well as waiting for its
//! response: a server that stops reading cannot block a call.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use grenat_sandbox::Sandboxed;
use serde_json::Value as Json;

use crate::group;
use crate::lines::lossy_lines;
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
    /// The lines to write on the server's standard input; dropped, it is closed.
    requests: Option<Sender<String>>,
    /// Whether each line was written.
    written: Receiver<bool>,
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
        group::lead(&mut command);
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start the process of facet `{}` (`{}`): {e}", launch.facet, argv.join(" ")))?;
        let stdin = child.stdin.take().expect("piped");
        let (stdout, stderr) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
        let (requests, written) = writer(stdin);
        let (send, responses) = channel();
        let facet = launch.facet.to_string();
        let log = launch.log.clone();
        std::thread::spawn(move || {
            for line in lossy_lines(stdout) {
                match protocol::response(&line) {
                    Some(response) => {
                        // the process is gone: its output is drained, unread
                        let _ = send.send(response);
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
            for line in lossy_lines(stderr) {
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
        Ok(Process { child, requests: Some(requests), written, responses, stderr: tail, stderr_done, next_id: 1 })
    }

    /// Whether the server still runs.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Sends a request, and waits `timeout` at most for it to be written
    /// and answered; a server that does not answer in time is killed.
    pub fn request(&mut self, method: &str, params: Option<Json>, timeout: Duration) -> Result<Reply, Failure> {
        let id = self.next_id;
        self.next_id += 1;
        // a timeout too long to be a date is no timeout
        let deadline = Instant::now().checked_add(timeout);
        let queued =
            self.requests.as_ref().is_some_and(|requests| requests.send(protocol::request(id, method, params)).is_ok());
        let written = if queued { receive(&self.written, deadline) } else { Ok(false) };
        match written {
            Ok(true) => {}
            Ok(false) | Err(RecvTimeoutError::Disconnected) => return Err(Failure::NotDelivered(self.death())),
            Err(RecvTimeoutError::Timeout) => {
                self.kill();
                return Err(Failure::TimedOut);
            }
        }
        loop {
            match receive(&self.responses, deadline) {
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

    /// Kills the server, and whatever it started.
    fn kill(&mut self) {
        group::kill(&mut self.child);
    }
}

impl Drop for Process {
    /// Standard input closed, the server ends by itself (the helper
    /// libraries do); one that does not is killed, with what it started.
    /// Its standard error is logged to the end.
    fn drop(&mut self) {
        drop(self.requests.take());
        let deadline = Instant::now() + Duration::from_millis(500);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.kill();
        let _ = self.stderr_done.recv_timeout(Duration::from_secs(1));
    }
}

/// The thread that writes requests on `stdin`, a line each: the lines to
/// write, and whether each was. It ends at the first that is not, or when
/// the lines' sender is dropped, closing `stdin`.
fn writer(mut stdin: ChildStdin) -> (Sender<String>, Receiver<bool>) {
    let (requests, lines) = channel::<String>();
    let (report, written) = channel();
    std::thread::spawn(move || {
        for line in lines {
            let ok = writeln!(stdin, "{line}").and_then(|()| stdin.flush()).is_ok();
            if report.send(ok).is_err() || !ok {
                break;
            }
        }
    });
    (requests, written)
}

/// The next message of `from`, waited for until `deadline` (forever without one).
fn receive<T>(from: &Receiver<T>, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
    match deadline {
        Some(deadline) => from.recv_timeout(deadline.saturating_duration_since(Instant::now())),
        None => from.recv().map_err(|_| RecvTimeoutError::Disconnected),
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
    fn writing_a_request_is_bounded_by_the_timeout() {
        // never reads: a request bigger than the pipe's buffer cannot be written
        let mut process = sh("sleep 30", None);
        let big = Json::from("x".repeat(1 << 20));
        let started = Instant::now();
        assert_eq!(process.request("call", Some(big), Duration::from_millis(300)), Err(Failure::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        assert!(!process.is_running());
    }

    #[test]
    fn a_timeout_too_long_to_be_a_date_is_no_timeout() {
        let mut process = sh(r#"read line; echo '{"jsonrpc":"2.0","id":1,"result":7}'; sleep 5"#, None);
        assert_eq!(process.request("call", None, Duration::MAX), Ok(Reply::Result(Json::from(7))));
    }

    #[test]
    fn standard_error_is_read_to_the_end_whatever_its_bytes() {
        let logged = Arc::new(Mutex::new(String::new()));
        let sink = logged.clone();
        // a byte that is not UTF-8, then more than a pipe holds: the server must not block, nor get EPIPE
        let script = r#"read line; printf 'caf\351\n' >&2; i=0; while [ $i -lt 20000 ]; do echo "line $i" >&2; i=$((i+1)); done; echo '{"jsonrpc":"2.0","id":1,"result":1}'; read line"#;
        let mut process = sh(script, Some(Arc::new(move |line: &str| sink.lock().unwrap().push_str(line))));
        assert_eq!(process.request("call", None, Duration::from_secs(20)), Ok(Reply::Result(Json::from(1))));
        drop(process);
        let logged = logged.lock().unwrap();
        assert!(logged.starts_with("[bridge] t: caf\u{fffd}\n[bridge] t: line 0\n"), "{}", &logged[..100]);
        assert!(logged.ends_with("[bridge] t: line 19999\n"));
    }

    #[test]
    fn a_server_killed_takes_what_it_started_along() {
        let pidfile = std::env::temp_dir().join(format!("grenat-bridge-wrapped-{}", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        // a wrapper that does not `exec` its server
        let mut process = sh(&format!("sh -c 'echo $$ > {}; exec sleep 30'; true", pidfile.display()), None);
        assert_eq!(process.request("call", None, Duration::from_millis(300)), Err(Failure::TimedOut));
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        let alive = || {
            std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let killed = Instant::now();
        while alive() && killed.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!alive(), "the server started by the wrapper still runs");
        let _ = std::fs::remove_file(&pidfile);
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
