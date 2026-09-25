//! Agentic loop of `run`: LLM ↔ tools until the typed final answer.

use crate::prelude::*;
use grenat_llm::{ModelConfig, Request, ToolSpec, ToolUse};
use serde_json::{Value as Json, json};

use super::*;

pub(crate) const DEFAULT_MAX_TURNS: usize = 20;
pub(crate) const FINAL_TOOL: &str = "final_answer";

pub(crate) struct AgentConfig<'p> {
    model: ModelConfig,
    tools: Vec<&'p str>,
    max_turns: usize,
    instructions: Option<String>,
    /// How long one tool call may take (`tool_timeout 30`).
    tool_timeout: Option<std::time::Duration>,
}

impl<'p> Interp<'p> {
    // ── Agents: the `run` loop ────────────────────────────────

    pub(crate) fn agent_config(&mut self, ty: &str) -> Result<AgentConfig<'p>, Ctrl<'p>> {
        let directives = self.types[ty].directives.clone();
        let mut model_selector = None;
        let mut config = AgentConfig {
            model: ModelConfig::new("", ""),
            tools: Vec::new(),
            max_turns: DEFAULT_MAX_TURNS,
            instructions: None,
            tool_timeout: None,
        };
        for directive in directives {
            let first = directive.args.iter().find_map(|a| match a {
                grenat_ast::Arg::Pos(e) => Some(e),
                _ => None,
            });
            match directive.name.name.as_str() {
                "model" => model_selector = first,
                "tools" => {
                    for arg in &directive.args {
                        match arg {
                            grenat_ast::Arg::Pos(Expr { kind: ExprKind::Var(name), .. }) => config.tools.push(name),
                            _ => {
                                return raise("TypeError", "`tools` expects tool names: `tools read, search`");
                            }
                        }
                    }
                }
                "max_turns" => match first.map(|e| self.eval(e)).transpose()? {
                    Some(Value::Int(n)) if n > 0 => config.max_turns = n as usize,
                    _ => return raise("TypeError", "`max_turns` expects a positive integer"),
                },
                "instructions" => {
                    if let Some(e) = first {
                        let v = self.eval(e)?;
                        config.instructions = Some(v.to_display());
                    }
                }
                "tool_timeout" => match first.map(|e| self.eval(e)).transpose()? {
                    Some(Value::Int(n)) if n > 0 => config.tool_timeout = Some(std::time::Duration::from_secs(n as u64)),
                    Some(Value::Float(s) | Value::Duration(s)) if s > 0.0 => {
                        config.tool_timeout = Some(std::time::Duration::from_secs_f64(s));
                    }
                    _ => return raise("TypeError", "`tool_timeout` expects a duration: `tool_timeout 30`"),
                },
                "budget" => {}
                other => return raise("NameError", format!("unknown agent directive `{other}`")),
            }
        }
        config.model = self.model(model_selector)?;
        Ok(config)
    }

    pub(crate) fn tool_spec(&self, def: &'p FnDef) -> Result<ToolSpec, String> {
        let fields = def.params.iter().map(|p| (p.name.name.as_str(), p.ty.as_ref(), None, p.default.is_some()));
        Ok(ToolSpec {
            name: def.name.name.clone(),
            description: def.doc.clone().unwrap_or_else(|| format!("Outil `{}`.", def.name.name)),
            input_schema: self.object_schema(fields, 0)?,
        })
    }

    /// `run "instruction"` in an agent handler: LLM ↔ tools loop until `final_answer`.
    pub(crate) fn agent_run(&mut self, args: Args<'p>) -> R<'p> {
        let frame = self.agents.last().expect("inside an agent");
        let (agent_ty, handler) = (frame.agent.ty.clone(), frame.handler);
        let Some(instruction) = args.pos.first() else {
            return raise("ArgumentError", "`run` expects an instruction: `run \"…\"`");
        };
        let instruction = instruction.to_display();
        let config = self.agent_config(&agent_ty)?;
        let ret = match &handler.ret {
            Some(t) => self.ty(t).or_else(type_error)?,
            None => Ty::Str,
        };
        let (final_schema, wrapped) = self.output_schema(&ret).or_else(type_error)?;

        let mut tools = Vec::new();
        for name in &config.tools {
            let Some(def) = self.fns.get(name).copied() else {
                return raise("NameError", format!("unknown tool `{name}` in the `tools` of `{agent_ty}`"));
            };
            if def.kind != FnKind::Tool {
                return raise("TypeError", format!("`{name}` must be declared with `tool` to be given to an agent"));
            }
            tools.push(self.tool_spec(def).or_else(type_error)?);
        }
        tools.push(ToolSpec {
            name: FINAL_TOOL.into(),
            description: "Give your final answer. Call this tool once, when you are done.".into(),
            input_schema: final_schema,
        });
        let system = format!(
            "{}\n\nWhen you are done, call the `{FINAL_TOOL}` tool with your final answer.",
            config.instructions.as_deref().unwrap_or_default()
        );
        let mut messages = vec![json!({"role": "user", "content": instruction})];

        for _turn in 0..config.max_turns {
            let request = Request {
                model: &config.model,
                system: Some(system.trim().to_string()),
                messages: messages.clone(),
                tools: tools.clone(),
                output_schema: None,
            };
            let response = self.llm_call(&request)?;
            messages.push(json!({"role": "assistant", "content": response.content}));
            let uses = response.tool_uses();
            if uses.is_empty() {
                messages.push(
                    json!({"role": "user", "content": format!("Call the `{FINAL_TOOL}` tool with your final answer.")}),
                );
                continue;
            }
            let mut results = Vec::new();
            for tool_use in &uses {
                if tool_use.name == FINAL_TOOL {
                    let payload = if wrapped { tool_use.input["value"].clone() } else { tool_use.input.clone() };
                    match self.json_to_value(&payload, &ret) {
                        Ok(value) => return Ok(value.taint()),
                        Err(e) => {
                            results.push(tool_result(&tool_use.id, format!("Invalid final answer: {e}"), true));
                            continue;
                        }
                    }
                }
                let (content, is_error) = match self.call_tool_within(tool_use, config.tool_timeout) {
                    Ok(v) => (tool_output(&v), false),
                    Err(Ctrl::Raise(e)) if !matches!(&*e.ty, "BudgetExceeded" | "TaintError" | "StackOverflow") => {
                        (format!("{}: {}", e.ty, e.message), true)
                    }
                    Err(other) => return Err(other),
                };
                results.push(tool_result(&tool_use.id, content, is_error));
            }
            messages.push(json!({"role": "user", "content": results}));
        }
        raise("MaxTurnsExceeded", format!("agent `{agent_ty}` did not finish within {} turns", config.max_turns))
    }

    /// A tool call, cancelled at its next checkpoint once `timeout` has
    /// passed: a `TimeoutError`, reported to the model like any tool error.
    fn call_tool_within(&mut self, tool_use: &ToolUse, timeout: Option<std::time::Duration>) -> R<'p> {
        let Some(timeout) = timeout else { return self.call_tool(tool_use) };
        let expired = crate::deadlines::after(timeout);
        self.cancel.push(expired.clone());
        let result = self.call_tool(tool_use);
        self.cancel.pop();
        match result {
            Err(Ctrl::Raise(e)) if &*e.ty == "Cancelled" && expired.load(AtomicOrdering::Relaxed) => raise(
                "TimeoutError",
                format!("tool `{}` took more than {}s", tool_use.name, timeout.as_secs_f64()),
            ),
            other => other,
        }
    }

    /// Runs a tool requested by the LLM: arguments are validated against the schema,
    /// hence untainted — the tool is the trust boundary.
    pub(crate) fn call_tool(&mut self, tool_use: &ToolUse) -> R<'p> {
        let Some(def) = self.fns.get(tool_use.name.as_str()).copied() else {
            return raise("NameError", format!("unknown tool `{}`", tool_use.name));
        };
        let mut args = Args::default();
        for param in &def.params {
            let json = tool_use.input.get(&param.name.name);
            if matches!(json, None | Some(Json::Null)) && param.default.is_some() {
                continue;
            }
            let Some(ty) = &param.ty else {
                return raise(
                    "TypeError",
                    format!("parameter `{}` of tool `{}` has no type", param.name.name, def.name.name),
                );
            };
            let ty = self.ty(ty).or_else(type_error)?;
            let value = self
                .json_to_value(json.unwrap_or(&Json::Null), &ty)
                .or_else(|e| raise("ArgumentError", format!("`{}`: {e}", param.name.name)))?;
            args.named.push((param.name.name.clone(), value));
        }
        if self.log {
            let line = format!("[tool] {}({})\n", def.name.name, tool_use.input);
            self.write_err(&line);
        }
        self.call_fn(def, args, None)
    }
}

pub(crate) fn tool_result(id: &str, content: String, is_error: bool) -> Json {
    json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error})
}

pub(crate) fn tool_output(value: &Value) -> String {
    match value.untainted() {
        Value::Str(s) => s.to_string(),
        Value::Nil => "ok".into(),
        other => value_to_json(other).to_string(),
    }
}
