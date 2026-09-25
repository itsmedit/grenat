//! MCP servers: their tools handed to agents, or called directly.
//!
//! ```ruby
//! mcp :linear, url: "https://mcp.linear.app/mcp", headers: {"Authorization" => "Bearer …"}
//! mcp :files, command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "./docs"]
//!
//! agent Pm
//!   tools mcp(:linear), mcp(:files, only: ["read_file"])
//! end
//!
//! Mcp.call(:linear, "list_issues", {query: "sso"})   # untrusted text
//! ```
//!
//! A server is started or reached when first used, under the `mcp("name")`
//! capability. Its tools keep the schemas it gives. A tool the server does
//! not declare read-only and non-destructive is approved by the human first
//! (`approve: false` on the server to skip that). Tests never reach a
//! server: `mock_mcp` answers for it.

use std::collections::HashMap;
use std::time::Duration;

use grenat_llm::ToolSpec;
use grenat_mcp::{Client, Tool};
use serde_json::Value as Json;

use crate::llm::value_to_json;
use crate::prelude::*;

/// How to reach a server.
pub(crate) enum Endpoint {
    Command { argv: Vec<String>, env: Vec<(String, String)> },
    Url { url: String, headers: Vec<(String, String)> },
}

/// A declared server, connected when first used.
pub(crate) struct McpServer {
    endpoint: Endpoint,
    /// Ask the human before a tool that may change things.
    approve: bool,
    client: grenat_green::Mutex<Option<Client>>,
    tools: Mutex<Option<Vec<Tool>>>,
}

/// Where a tool handed to a model is: (server, tool).
pub(crate) type Route = (String, String);

/// What `mock_mcp` answers for a server's tools: tool → text.
pub(crate) type McpStub = HashMap<String, String>;

/// The name under which a server's tool is handed to a model.
pub(crate) fn exposed(server: &str, tool: &str) -> String {
    let name: String =
        format!("{server}__{tool}").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    name.chars().take(64).collect()
}

impl<'p> Interp<'p> {
    /// `mcp :name, url: "…", headers: {…}` or `command: [...], env: {…}`, `approve:`.
    pub(crate) fn declare_mcp(&mut self, args: &Args<'p>) -> R<'p> {
        let Some(Value::Symbol(name)) = args.pos.first().map(Value::untainted) else {
            return raise("ArgumentError", "`mcp` expects a name: `mcp :linear, url: \"…\"`");
        };
        let strings = |v: &Value<'p>| -> Vec<(String, String)> {
            match v.untainted() {
                Value::Hash(h) => h.borrow().iter().map(|(k, v)| (k.to_display(), v.to_display())).collect(),
                _ => Vec::new(),
            }
        };
        let (mut url, mut argv, mut headers, mut env, mut approve) = (None, None::<Vec<String>>, Vec::new(), Vec::new(), true);
        for (option, value) in &args.named {
            if value.contains_taint() {
                return raise("TaintError", format!("an untrusted value configures the MCP server `{name}`"));
            }
            match (option.as_str(), value) {
                ("url", Value::Str(u)) => url = Some(u.to_string()),
                ("command", Value::Array(items)) => argv = Some(items.borrow().iter().map(Value::to_display).collect()),
                ("headers", v @ Value::Hash(_)) => headers = strings(v),
                ("env", v @ Value::Hash(_)) => env = strings(v),
                ("approve", Value::Bool(b)) => approve = *b,
                (option, v) => return raise("ArgumentError", format!("invalid `mcp` option `{option}: {}`", v.inspect())),
            }
        }
        let endpoint = match (url, argv) {
            (Some(url), None) => Endpoint::Url { url, headers },
            (None, Some(argv)) if !argv.is_empty() => Endpoint::Command { argv, env },
            _ => return raise("ArgumentError", format!("MCP server `{name}`: give either `url:` or `command:`")),
        };
        let server = McpServer { endpoint, approve, client: grenat_green::Mutex::new(None), tools: Mutex::new(None) };
        self.mcp_servers.borrow_mut().insert(name.to_string(), Arc::new(server));
        Ok(Value::Nil)
    }

    /// `mock_mcp :linear, tools: {"create_issue" => "created L-1"}`: the
    /// server's tools and what they answer, until the end of the test.
    pub(crate) fn mock_mcp(&mut self, args: &Args<'p>) -> R<'p> {
        let Some(Value::Symbol(name)) = args.pos.first().map(Value::untainted) else {
            return raise("ArgumentError", "`mock_mcp` expects a server: `mock_mcp :linear, tools: {…}`");
        };
        let Some((_, Value::Hash(tools))) = args.named.iter().find(|(n, _)| n == "tools") else {
            return raise("ArgumentError", "`mock_mcp` expects `tools: {\"name\" => \"answer\"}`");
        };
        let stub = tools.borrow().iter().map(|(k, v)| (k.to_display(), v.to_display())).collect();
        self.mcp_stubs.borrow_mut().insert(name.to_string(), stub);
        Ok(Value::Nil)
    }

    fn server(&self, name: &str) -> Result<Arc<McpServer>, Ctrl<'p>> {
        match self.mcp_servers.borrow().get(name) {
            Some(server) => Ok(server.clone()),
            None => raise("NameError", format!("MCP server `:{name}` is not declared: `mcp :{name}, url: \"…\"`")),
        }
    }

    /// The tools of a server (listed once), stubs in tests.
    pub(crate) fn mcp_tools(&mut self, name: &str) -> Result<Vec<Tool>, Ctrl<'p>> {
        self.check_mcp(name)?;
        if let Some(stub) = self.mcp_stubs.borrow().get(name) {
            let mut names: Vec<&String> = stub.keys().collect();
            names.sort();
            return Ok(names
                .into_iter()
                .map(|n| Tool {
                    name: n.clone(),
                    description: format!("`{n}` (mocked)"),
                    input_schema: serde_json::json!({"type": "object"}),
                    read_only: true,
                    destructive: false,
                })
                .collect());
        }
        let server = self.server(name)?;
        if let Some(tools) = server.tools.borrow().clone() {
            return Ok(tools);
        }
        let tools = self.with_client(name, &server, |client| client.tools())?;
        *server.tools.borrow_mut() = Some(tools.clone());
        Ok(tools)
    }

    /// The tools of `name` (only those in `only`, if given), as a model sees them.
    pub(crate) fn mcp_tool_specs(
        &mut self,
        name: &str,
        only: Option<&[String]>,
    ) -> Result<Vec<(ToolSpec, Route)>, Ctrl<'p>> {
        let tools = self.mcp_tools(name)?;
        if let Some(only) = only
            && let Some(missing) = only.iter().find(|o| !tools.iter().any(|t| &t.name == *o))
        {
            return raise("NameError", format!("the MCP server `:{name}` has no tool `{missing}`"));
        }
        Ok(tools
            .into_iter()
            .filter(|t| only.is_none_or(|o| o.contains(&t.name)))
            .map(|t| {
                let spec = ToolSpec {
                    name: exposed(name, &t.name),
                    description: format!("[{name}] {}", t.description),
                    input_schema: t.input_schema.clone(),
                    strict: false,
                };
                (spec, (name.to_string(), t.name))
            })
            .collect())
    }

    /// Calls a server's tool: `(text, is_error)`. A tool that may change
    /// things is approved by the human first, unless the server says not to.
    pub(crate) fn call_mcp(&mut self, name: &str, tool: &str, input: &Json) -> Result<(String, bool), Ctrl<'p>> {
        let stubbed = self.mcp_stubs.borrow().get(name).map(|stub| stub.get(tool).cloned());
        match stubbed {
            Some(Some(answer)) => {
                self.check_mcp(name)?;
                return Ok((answer, false));
            }
            Some(None) => return raise("McpError", format!("the mock of `:{name}` has no tool `{tool}`")),
            None => {}
        }
        let server = self.server(name)?;
        let info = self.mcp_tools(name)?.into_iter().find(|t| t.name == tool);
        let Some(info) = info else { return raise("McpError", format!("the MCP server `:{name}` has no tool `{tool}`")) };
        if server.approve && (info.destructive || !info.read_only) {
            let request = format!("Allow the MCP tool `{name}.{tool}` with {input}?");
            if !self.ask_human(&request)? {
                return raise("ApprovalDenied", format!("rejected by the human: {request}"));
            }
        }
        if self.log {
            self.write_err(&format!("[mcp] {name}.{tool}({input})\n"));
        }
        let result = self.with_client(name, &server, |client| client.call(tool, input.clone()))?;
        Ok((result.text, result.is_error))
    }

    /// `Mcp.call(:server, "tool", {…})`: the tool's text, untrusted.
    pub(crate) fn mcp_call_value(&mut self, args: &Args<'p>) -> R<'p> {
        let (Some(Value::Symbol(name)), Some(tool)) = (args.pos.first().map(Value::untainted), args.pos.get(1)) else {
            return raise("ArgumentError", "`Mcp.call` expects a server and a tool: `Mcp.call(:linear, \"list_issues\", {…})`");
        };
        if args.pos.iter().any(Value::contains_taint) {
            return raise("TaintError", "an untrusted value reaches `Mcp.call` (effect `mcp`) without validation");
        }
        let input = args.pos.get(2).map_or_else(|| serde_json::json!({}), value_to_json);
        let (text, is_error) = self.call_mcp(&name.clone(), &tool.to_display(), &input)?;
        if is_error {
            return raise("McpError", text);
        }
        Ok(Value::str(text).taint())
    }

    /// Runs `f` with the connected client of `name` (connecting first).
    fn with_client<T: Send>(
        &self,
        name: &str,
        server: &McpServer,
        f: impl FnOnce(&mut Client) -> Result<T, String> + Send,
    ) -> Result<T, Ctrl<'p>> {
        if self.offline {
            return raise("McpError", format!("no MCP servers in tests: `:{name}` is not stubbed with `mock_mcp`"));
        }
        let mut client = server.client.lock();
        let result = grenat_green::blocking(|| {
            if client.is_none() {
                let transport: Box<dyn grenat_mcp::Transport> = match &server.endpoint {
                    Endpoint::Command { argv, env } => Box::new(grenat_mcp::Stdio::spawn(argv, env)?),
                    Endpoint::Url { url, headers } => Box::new(grenat_mcp::Http::new(url, headers, Duration::from_secs(60))),
                };
                *client = Some(Client::connect(transport)?);
            }
            f(client.as_mut().expect("connected"))
        });
        result.or_else(|e| raise("McpError", format!("`:{name}`: {e}")))
    }
}
