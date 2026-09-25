//! The session with a server: initialization, its tools, calls.

use serde_json::{Value as Json, json};

use crate::transport::Transport;

/// A tool offered by a server.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Json,
    /// The server says it changes nothing.
    pub read_only: bool,
    /// The server says it may destroy or overwrite things.
    pub destructive: bool,
}

/// What a call returned: its text content, and whether it is an error the
/// tool reports (as opposed to a failure of the protocol).
#[derive(Debug, Clone, PartialEq)]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
}

pub struct Client {
    transport: Box<dyn Transport>,
    next_id: u64,
    /// The server's name, as it introduced itself.
    pub server: String,
}

impl Client {
    /// Opens a session over `transport`.
    pub fn connect(transport: Box<dyn Transport>) -> Result<Client, String> {
        let mut client = Client { transport, next_id: 1, server: String::new() };
        let result = client.call_method(
            "initialize",
            json!({
                "protocolVersion": crate::PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "grenat", "version": env!("CARGO_PKG_VERSION")},
            }),
        )?;
        client.server = result["serverInfo"]["name"].as_str().unwrap_or("MCP server").to_string();
        client.transport.notify(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(client)
    }

    /// Every tool of the server (all pages).
    pub fn tools(&mut self) -> Result<Vec<Tool>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let result = self.call_method("tools/list", params)?;
            for tool in result["tools"].as_array().cloned().unwrap_or_default() {
                let hint = |name: &str| tool["annotations"][name].as_bool();
                tools.push(Tool {
                    name: tool["name"].as_str().unwrap_or_default().to_string(),
                    description: tool["description"].as_str().unwrap_or_default().to_string(),
                    input_schema: tool["inputSchema"].clone(),
                    read_only: hint("readOnlyHint").unwrap_or(false),
                    // the protocol's default: a tool that is not read-only may be destructive
                    destructive: !hint("readOnlyHint").unwrap_or(false) && hint("destructiveHint").unwrap_or(true),
                });
            }
            cursor = result["nextCursor"].as_str().map(str::to_string);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }

    /// Calls the tool `name` with `arguments` (a JSON object).
    pub fn call(&mut self, name: &str, arguments: Json) -> Result<CallResult, String> {
        let result = self.call_method("tools/call", json!({"name": name, "arguments": arguments}))?;
        let text = result["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .map(|b| match b["type"].as_str() {
                        Some("text") => b["text"].as_str().unwrap_or_default().to_string(),
                        Some(other) => format!("[{other} content]"),
                        None => String::new(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let text = if text.is_empty() && !result["structuredContent"].is_null() {
            result["structuredContent"].to_string()
        } else {
            text
        };
        Ok(CallResult { text, is_error: result["isError"].as_bool().unwrap_or(false) })
    }

    fn call_method(&mut self, method: &str, params: Json) -> Result<Json, String> {
        let id = self.next_id;
        self.next_id += 1;
        let reply = self.transport.request(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        match reply.get("error") {
            Some(error) => Err(format!("{method}: {}", error["message"].as_str().unwrap_or("error"))),
            None => Ok(reply["result"].clone()),
        }
    }
}
