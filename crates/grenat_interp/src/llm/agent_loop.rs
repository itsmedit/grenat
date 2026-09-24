//! Boucle agentique de `run` : LLM ↔ outils jusqu'à la réponse finale typée.

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
}

impl<'p> Interp<'p> {
    // ── Agents : boucle `run` ────────────────────────────────

    pub(crate) fn agent_config(&mut self, ty: &str) -> Result<AgentConfig<'p>, Ctrl<'p>> {
        let directives = self.types[ty].directives.clone();
        let mut model_selector = None;
        let mut config = AgentConfig {
            model: ModelConfig::new("", ""),
            tools: Vec::new(),
            max_turns: DEFAULT_MAX_TURNS,
            instructions: None,
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
                                return raise("TypeError", "`tools` attend des noms d'outils : `tools lire, chercher`");
                            }
                        }
                    }
                }
                "max_turns" => match first.map(|e| self.eval(e)).transpose()? {
                    Some(Value::Int(n)) if n > 0 => config.max_turns = n as usize,
                    _ => return raise("TypeError", "`max_turns` attend un entier positif"),
                },
                "instructions" => {
                    if let Some(e) = first {
                        let v = self.eval(e)?;
                        config.instructions = Some(v.to_display());
                    }
                }
                "budget" => {}
                other => return raise("NameError", format!("directive d'agent inconnue `{other}`")),
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

    /// `run "consigne"` dans un handler d'agent : boucle LLM ↔ outils jusqu'à `final_answer`.
    pub(crate) fn agent_run(&mut self, args: Args<'p>) -> R<'p> {
        let frame = self.agents.last().expect("dans un agent");
        let (agent_ty, handler) = (frame.agent.ty.clone(), frame.handler);
        let Some(instruction) = args.pos.first() else {
            return raise("ArgumentError", "`run` attend une consigne : `run \"…\"`");
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
                return raise("NameError", format!("outil inconnu `{name}` dans `tools` de `{agent_ty}`"));
            };
            if def.kind != FnKind::Tool {
                return raise(
                    "TypeError",
                    format!("`{name}` doit être déclaré avec `tool` pour être confié à un agent"),
                );
            }
            tools.push(self.tool_spec(def).or_else(type_error)?);
        }
        tools.push(ToolSpec {
            name: FINAL_TOOL.into(),
            description: "Donne ta réponse finale. Appelle cet outil une seule fois, quand tu as terminé.".into(),
            input_schema: final_schema,
        });
        let system = format!(
            "{}\n\nQuand tu as terminé, appelle l'outil `{FINAL_TOOL}` avec ta réponse finale.",
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
                messages.push(json!({"role": "user", "content": format!("Appelle l'outil `{FINAL_TOOL}` avec ta réponse finale.")}));
                continue;
            }
            let mut results = Vec::new();
            for tool_use in &uses {
                if tool_use.name == FINAL_TOOL {
                    let payload = if wrapped { tool_use.input["value"].clone() } else { tool_use.input.clone() };
                    match self.json_to_value(&payload, &ret) {
                        Ok(value) => return Ok(value.taint()),
                        Err(e) => {
                            results.push(tool_result(&tool_use.id, format!("Réponse finale invalide : {e}"), true));
                            continue;
                        }
                    }
                }
                let (content, is_error) = match self.call_tool(tool_use) {
                    Ok(v) => (tool_output(&v), false),
                    Err(Ctrl::Raise(e)) if !matches!(&*e.ty, "BudgetExceeded" | "TaintError" | "StackOverflow") => {
                        (format!("{} : {}", e.ty, e.message), true)
                    }
                    Err(other) => return Err(other),
                };
                results.push(tool_result(&tool_use.id, content, is_error));
            }
            messages.push(json!({"role": "user", "content": results}));
        }
        raise("MaxTurnsExceeded", format!("l'agent `{agent_ty}` n'a pas conclu en {} tours", config.max_turns))
    }

    /// Exécute un outil demandé par le LLM : arguments validés contre le schéma,
    /// donc non teintés — l'outil est la frontière de confiance.
    pub(crate) fn call_tool(&mut self, tool_use: &ToolUse) -> R<'p> {
        let Some(def) = self.fns.get(tool_use.name.as_str()).copied() else {
            return raise("NameError", format!("outil inconnu `{}`", tool_use.name));
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
                    format!("paramètre `{}` de l'outil `{}` sans type", param.name.name, def.name.name),
                );
            };
            let ty = self.ty(ty).or_else(type_error)?;
            let value = self
                .json_to_value(json.unwrap_or(&Json::Null), &ty)
                .or_else(|e| raise("ArgumentError", format!("`{}` : {e}", param.name.name)))?;
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
