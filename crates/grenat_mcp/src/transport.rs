//! How JSON-RPC messages reach a server: its standard input and output, or
//! HTTP.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio as Pipe};
use std::time::Duration;

use serde_json::Value as Json;

pub trait Transport: Send {
    /// Sends a request (it has an `id`) and returns the response with that id.
    fn request(&mut self, message: &Json) -> Result<Json, String>;
    /// Sends a notification (no response).
    fn notify(&mut self, message: &Json) -> Result<(), String>;
}

/// A server run as a program: one JSON message per line.
pub struct Stdio {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Stdio {
    /// Starts `argv`, with `env` added to the environment.
    pub fn spawn(argv: &[String], env: &[(String, String)]) -> Result<Stdio, String> {
        let (program, args) = argv.split_first().ok_or("an MCP server command cannot be empty")?;
        let mut child = Command::new(program)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Pipe::piped())
            .stdout(Pipe::piped())
            .stderr(Pipe::null())
            .spawn()
            .map_err(|e| format!("cannot start the MCP server `{program}`: {e}"))?;
        let input = child.stdin.take().ok_or("no standard input")?;
        let output = BufReader::new(child.stdout.take().ok_or("no standard output")?);
        Ok(Stdio { child, input, output })
    }

    fn send(&mut self, message: &Json) -> Result<(), String> {
        writeln!(self.input, "{message}").and_then(|()| self.input.flush()).map_err(|e| format!("the MCP server stopped: {e}"))
    }
}

impl Transport for Stdio {
    fn request(&mut self, message: &Json) -> Result<Json, String> {
        self.send(message)?;
        loop {
            let mut line = String::new();
            if self.output.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err("the MCP server closed its output".into());
            }
            // notifications and requests from the server are skipped
            if let Ok(reply) = serde_json::from_str::<Json>(&line)
                && reply.get("id") == message.get("id")
                && reply.get("method").is_none()
            {
                return Ok(reply);
            }
        }
    }

    fn notify(&mut self, message: &Json) -> Result<(), String> {
        self.send(message)
    }
}

impl Drop for Stdio {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A server at a URL (streamable HTTP): a POST per message, answered with
/// JSON or with server-sent events.
pub struct Http {
    url: String,
    headers: Vec<(String, String)>,
    session: Option<String>,
    agent: ureq::Agent,
}

impl Http {
    pub fn new(url: &str, headers: &[(String, String)], timeout: Duration) -> Http {
        let agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(timeout)).build().into();
        Http { url: url.to_string(), headers: headers.to_vec(), session: None, agent }
    }

    fn post(&mut self, message: &Json) -> Result<(u16, String), String> {
        let mut request = self
            .agent
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", crate::PROTOCOL_VERSION);
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        if let Some(session) = &self.session {
            request = request.header("Mcp-Session-Id", session);
        }
        let mut response = request.send(message.to_string()).map_err(|e| format!("{}: {e}", self.url))?;
        if let Some(session) = response.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session = Some(session.to_string());
        }
        let status = response.status().as_u16();
        let body = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok((status, body))
    }
}

impl Transport for Http {
    fn request(&mut self, message: &Json) -> Result<Json, String> {
        let (status, body) = self.post(message)?;
        if !(200..300).contains(&status) {
            return Err(format!("the MCP server answered HTTP {status}: {}", body.trim()));
        }
        // JSON, or events whose `data:` lines are messages
        let messages: Vec<Json> = match serde_json::from_str::<Json>(&body) {
            Ok(json) => vec![json],
            Err(_) => body
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|data| serde_json::from_str(data.trim()).ok())
                .collect(),
        };
        messages
            .into_iter()
            .find(|m| m.get("id") == message.get("id") && m.get("method").is_none())
            .ok_or_else(|| format!("no response to the request from {}", self.url))
    }

    fn notify(&mut self, message: &Json) -> Result<(), String> {
        self.post(message).map(drop)
    }
}
