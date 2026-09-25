//! Tools and agents served to other programs: as an MCP server (streamable
//! HTTP) and as plain JSON endpoints, next to the routes.
//!
//! ```ruby
//! expose "/mcp", tools: [:find_ticket], agents: [Triage], token: Env.fetch("API_TOKEN")
//! ```
//!
//! - `POST /mcp`: MCP messages (`initialize`, `tools/list`, `tools/call`…);
//! - `GET /mcp`: the tools and their input schemas, as JSON;
//! - `POST /mcp/find_ticket` with `{"id": 42}`: `{"result": …}`.
//!
//! Each handler of an exposed agent is a tool (`triage_classify` for
//! `on Classify` of `Triage`), asked of one instance of the agent. Arguments
//! are checked against the schema; a tool is the trust boundary, as when a
//! model calls it, while an agent's message arrives untrusted. A token is
//! required (`Authorization: Bearer …`) unless the exposure says
//! `public: true`.

use std::collections::HashSet;

use grenat_ast::Handler;
use grenat_mcp::Tool;
use grenat_mcp::server::{Answer, Host};
use serde_json::{Value as Json, json};

use crate::builtins::{HttpAnswer, RawRequest};
use crate::llm::type_error;
use crate::prelude::*;

/// An `expose` declaration.
pub(crate) struct Exposure<'p> {
    /// Where it is served, without a trailing `/`.
    pub path: String,
    /// The MCP server's name.
    pub name: String,
    /// The bearer token callers present (`None`: public).
    pub token: Option<String>,
    pub entries: Vec<Exposed<'p>>,
}

/// A tool of an exposure: its description for callers, and what it runs.
pub(crate) struct Exposed<'p> {
    pub spec: Tool,
    pub target: Target<'p>,
}

pub(crate) enum Target<'p> {
    Tool(&'p FnDef),
    Handler { agent: Arc<AgentRef<'p>>, handler: &'p Handler },
}

/// Effects of a tool that changes nothing.
const READ_ONLY_EFFECTS: [&str; 5] = ["llm", "db.read", "fs.read", "env", "time"];

/// What a call gave: its value (JSON, and as the text a model would read), or
/// the error the tool raised.
enum Outcome {
    Value(Json, String),
    Failed(String),
}

impl<'p> Interp<'p> {
    /// `expose "/path", tools: [:name], agents: [Agent], token: "…"` (or
    /// `public: true`), `name: "support"`.
    pub(crate) fn declare_exposure(&mut self, args: &Args<'p>) -> R<'p> {
        let path = builtins::str_arg(args, 0, "expose")?.trim_end_matches('/').to_string();
        if !path.starts_with('/') || path.contains(':') {
            return raise("ArgumentError", format!("`expose` takes a path such as \"/mcp\", got {path:?}"));
        }
        let mut exposure = Exposure { path, name: "grenat".into(), token: None, entries: Vec::new() };
        let mut public = false;
        for (option, value) in &args.named {
            match (option.as_str(), value.untainted()) {
                ("tools", Value::Array(items)) => {
                    let items = items.borrow().clone();
                    for item in &items {
                        exposure.entries.push(self.exposed_tool(item)?);
                    }
                }
                ("agents", Value::Array(items)) => {
                    let items = items.borrow().clone();
                    for item in &items {
                        exposure.entries.extend(self.exposed_agent(item)?);
                    }
                }
                ("token", Value::Str(token)) if !token.is_empty() => exposure.token = Some(token.to_string()),
                ("public", Value::Bool(b)) => public = *b,
                ("name", Value::Str(name)) => exposure.name = name.to_string(),
                (option, v) => return raise("ArgumentError", format!("invalid `expose` option `{option}: {}`", v.inspect())),
            }
        }
        if exposure.token.is_none() && !public {
            return raise(
                "ArgumentError",
                format!("`expose {:?}` serves anyone: give it a `token:`, or say `public: true`", exposure.path),
            );
        }
        let mut names = HashSet::new();
        if let Some(twice) = exposure.entries.iter().find(|e| !names.insert(e.spec.name.clone())) {
            return raise("ArgumentError", format!("`expose` offers two tools named `{}`", twice.spec.name));
        }
        if self.exposures.borrow().iter().any(|e| e.path == exposure.path) {
            return raise("ArgumentError", format!("`{}` is exposed twice", exposure.path));
        }
        self.exposures.borrow_mut().push(Arc::new(exposure));
        Ok(Value::Nil)
    }

    fn exposed_tool(&self, item: &Value<'p>) -> Result<Exposed<'p>, Ctrl<'p>> {
        let Value::Symbol(name) = item.untainted() else {
            return raise("TypeError", format!("`tools:` expects tool names (`:search`), got {}", item.inspect()));
        };
        let Some(def) = self.fns.get(&**name).copied().filter(|d| d.kind == FnKind::Tool) else {
            return raise("NameError", format!("`{name}` is not a tool (declared with `tool`)"));
        };
        let spec = self.tool_spec(def).or_else(type_error)?;
        let read_only = def.effects.iter().all(|e| {
            let path: Vec<&str> = e.path.iter().map(|i| i.name.as_str()).collect();
            READ_ONLY_EFFECTS.contains(&path.join(".").as_str())
        });
        let spec = Tool {
            name: spec.name,
            description: spec.description,
            input_schema: spec.input_schema,
            read_only,
            destructive: !read_only,
        };
        Ok(Exposed { spec, target: Target::Tool(def) })
    }

    /// One tool per handler of the agent, asked of one instance of it.
    fn exposed_agent(&mut self, item: &Value<'p>) -> Result<Vec<Exposed<'p>>, Ctrl<'p>> {
        let agent = self.spawn_agent(item, None)?;
        let shared = self.shared.clone();
        let mut handlers: Vec<&'p Handler> = shared.types[&*agent.ty].handlers.values().copied().collect();
        handlers.sort_by_key(|h| h.span.start);
        let mut entries = Vec::new();
        for handler in handlers {
            let fields = handler.params.iter().map(|p| (p.name.name.as_str(), p.ty.as_ref(), None, p.default.is_some()));
            let input_schema = self.object_schema(fields, 0).or_else(type_error)?;
            let spec = Tool {
                name: format!("{}_{}", snake_case(&agent.ty), snake_case(&handler.message.name)),
                description: handler
                    .doc
                    .clone()
                    .unwrap_or_else(|| format!("Asks the agent `{}` a `{}` message.", agent.ty, handler.message.name)),
                input_schema,
                read_only: false,
                destructive: true,
            };
            entries.push(Exposed { spec, target: Target::Handler { agent: agent.clone(), handler } });
        }
        Ok(entries)
    }

    /// The answer to a request under an exposed path; `None` for other paths.
    pub(crate) fn handle_exposed(&mut self, raw: &RawRequest) -> Option<Result<HttpAnswer, Ctrl<'p>>> {
        let exposure = self
            .exposures
            .borrow()
            .iter()
            .find(|e| raw.path == e.path || raw.path.strip_prefix(&e.path).is_some_and(|rest| rest.starts_with('/')))
            .cloned()?;
        let header = |name: &str| raw.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
        if let Some(token) = &exposure.token
            && !grenat_serve::signature::bearer_valid(token, header("Authorization"))
        {
            let mut answer = HttpAnswer::text(401, "unauthorized");
            answer.headers.push(("WWW-Authenticate".into(), "Bearer".into()));
            return Some(Ok(answer));
        }
        let tool = raw.path[exposure.path.len()..].trim_matches('/').to_string();
        Some(match (tool.as_str(), raw.method.as_str()) {
            ("", "POST") => self.answer_mcp(&exposure, &raw.body),
            // a client asking for a stream of events: this server sends none
            ("", "GET") if header("Accept").is_some_and(|a| a.contains("text/event-stream")) => Ok(not_allowed()),
            ("", "GET") => {
                let tools: Vec<Json> = exposure
                    .entries
                    .iter()
                    .map(|e| json!({"name": e.spec.name, "description": e.spec.description, "input_schema": e.spec.input_schema}))
                    .collect();
                Ok(json_answer(200, &json!({"name": exposure.name, "tools": tools})))
            }
            (_, "POST") => self.answer_call(&exposure, &tool, &raw.body),
            _ => Ok(not_allowed()),
        })
    }

    fn answer_mcp(&mut self, exposure: &Exposure<'p>, body: &[u8]) -> Result<HttpAnswer, Ctrl<'p>> {
        let mut host = Served { interp: self, exposure, failure: None };
        let reply = grenat_mcp::server::handle_body(&mut host, body);
        if let Some(ctrl) = host.failure {
            return Err(ctrl);
        }
        Ok(match reply {
            Some(reply) => json_answer(200, &reply),
            None => HttpAnswer::text(202, ""),
        })
    }

    /// `POST /path/tool` with the arguments as a JSON object.
    fn answer_call(&mut self, exposure: &Exposure<'p>, tool: &str, body: &[u8]) -> Result<HttpAnswer, Ctrl<'p>> {
        let Some(entry) = exposure.entries.iter().find(|e| e.spec.name == tool) else {
            return Ok(json_answer(404, &json!({"error": format!("unknown tool `{tool}`")})));
        };
        let arguments = if body.iter().all(u8::is_ascii_whitespace) {
            json!({})
        } else {
            match serde_json::from_slice::<Json>(body) {
                Ok(arguments) => arguments,
                Err(e) => return Ok(json_answer(400, &json!({"error": format!("invalid JSON: {e}")}))),
            }
        };
        if !arguments.is_object() {
            return Ok(json_answer(400, &json!({"error": "the arguments are a JSON object"})));
        }
        Ok(match self.call_exposed(entry, &arguments)? {
            Outcome::Value(value, _) => json_answer(200, &json!({"result": value})),
            Outcome::Failed(error) => json_answer(422, &json!({"error": error})),
        })
    }

    fn call_exposed(&mut self, entry: &Exposed<'p>, arguments: &Json) -> Result<Outcome, Ctrl<'p>> {
        let result = match &entry.target {
            Target::Tool(def) => {
                let call = grenat_llm::ToolUse { id: "exposed".into(), name: def.name.name.clone(), input: arguments.clone() };
                self.call_tool(&call)
            }
            Target::Handler { agent, handler } => match self.exposed_message(handler, arguments) {
                Ok(message) => {
                    let args = Args { pos: vec![message], ..Args::default() };
                    self.send(&Value::Agent(agent.clone()), "ask", args).expect("an agent answers `ask`")
                }
                Err(e) => Err(e),
            },
        };
        if self.log {
            let status = if result.is_ok() { "ok" } else { "failed" };
            self.write_err(&format!("[expose] {} → {status}\n", entry.spec.name));
        }
        match result {
            Ok(value) => Ok(Outcome::Value(crate::llm::value_to_json(&value), crate::llm::tool_output(&value))),
            Err(Ctrl::Raise(e)) => Ok(Outcome::Failed(format!("{}: {}", e.ty, e.message))),
            Err(other) => Err(other),
        }
    }

    /// The message `arguments` describe, its values untrusted.
    fn exposed_message(&mut self, handler: &'p Handler, arguments: &Json) -> R<'p> {
        let mut fields = Vec::new();
        for param in &handler.params {
            let json = arguments.get(&param.name.name);
            if matches!(json, None | Some(Json::Null)) && param.default.is_some() {
                continue;
            }
            let Some(ty) = &param.ty else {
                return raise("TypeError", format!("field `{}` of `{}` has no type", param.name.name, handler.message.name));
            };
            let ty = self.ty(ty).or_else(type_error)?;
            let value = self
                .json_to_value(json.unwrap_or(&Json::Null), &ty)
                .or_else(|e| raise("ArgumentError", format!("`{}`: {e}", param.name.name)))?;
            fields.push((param.name.name.as_str().into(), value.taint()));
        }
        Ok(Value::record(&handler.message.name, fields))
    }
}

/// An exposure answering MCP messages.
struct Served<'a, 'p> {
    interp: &'a mut Interp<'p>,
    exposure: &'a Exposure<'p>,
    /// What stopped a call other than an error it raised (an `exit`…).
    failure: Option<Ctrl<'p>>,
}

impl Host for Served<'_, '_> {
    fn name(&self) -> String {
        self.exposure.name.clone()
    }

    fn tools(&mut self) -> Vec<Tool> {
        self.exposure.entries.iter().map(|e| e.spec.clone()).collect()
    }

    fn call(&mut self, name: &str, arguments: &Json) -> Result<Answer, String> {
        let Some(entry) = self.exposure.entries.iter().find(|e| e.spec.name == name) else {
            return Err(format!("unknown tool `{name}`"));
        };
        if !arguments.is_object() {
            return Err("the arguments are a JSON object".into());
        }
        match self.interp.call_exposed(entry, arguments) {
            Ok(Outcome::Value(value, text)) => {
                Ok(Answer { structured: value.is_object().then_some(value), text, is_error: false })
            }
            Ok(Outcome::Failed(error)) => Ok(Answer { text: error, structured: None, is_error: true }),
            Err(ctrl) => {
                self.failure = Some(ctrl);
                Err("the call was stopped".into())
            }
        }
    }
}

fn json_answer(status: u16, body: &Json) -> HttpAnswer {
    HttpAnswer { status, content_type: "application/json".into(), headers: Vec::new(), body: body.to_string() }
}

fn not_allowed() -> HttpAnswer {
    let mut answer = HttpAnswer::text(405, "method not allowed");
    answer.headers.push(("Allow".into(), "GET, POST".into()));
    answer
}

/// `AddNote` → `add_note`.
fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn tool_names_of_handlers() {
        assert_eq!(super::snake_case("AddNote"), "add_note");
        assert_eq!(super::snake_case("Triage"), "triage");
    }
}
