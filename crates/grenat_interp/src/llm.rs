//! Pont entre le langage et les LLM : schémas JSON générés depuis les types,
//! exécution des `prompt`, boucle agentique de `run`.

use std::rc::Rc;
use std::time::Instant;

use grenat_ast::{Expr, ExprKind, FnDef, FnKind, Type, TypeKind};
use grenat_llm::{Anthropic, ModelConfig, Request, Response, ToolSpec, ToolUse, cost_usd};
use serde_json::{Map, Value as Json, json};

use crate::value::{Fields, Value, Variant, money};
use crate::{Args, Ctrl, Interp, R, raise};

const MAX_SCHEMA_DEPTH: usize = 16;
const DEFAULT_MAX_TURNS: usize = 20;
const FINAL_TOOL: &str = "final_answer";

/// Type tel que vu par les LLM (la teinte `~` est ajoutée par le runtime).
#[derive(Debug, Clone)]
pub(crate) enum Ty<'p> {
    Int,
    Float,
    Str,
    Bool,
    Nil,
    Array(Box<Ty<'p>>),
    Hash(Box<Ty<'p>>),
    Opt(Box<Ty<'p>>),
    User(&'p str),
}

pub(crate) fn type_name(ty: &Type) -> &str {
    match ty {
        Type::Named { path, .. } => &path.last().expect("chemin non vide").name,
        Type::Optional(inner, _) | Type::Tainted(inner, _) => type_name(inner),
    }
}

struct AgentConfig<'p> {
    model: ModelConfig,
    tools: Vec<&'p str>,
    max_turns: usize,
    instructions: Option<String>,
}

fn type_error<'p, T>(message: String) -> Result<T, Ctrl<'p>> {
    raise("TypeError", message)
}

impl<'p> Interp<'p> {
    // ── Types et schémas ─────────────────────────────────────

    pub(crate) fn ty(&self, ty: &'p Type) -> Result<Ty<'p>, String> {
        match ty {
            Type::Tainted(inner, _) => self.ty(inner),
            Type::Optional(inner, _) => Ok(Ty::Opt(Box::new(self.ty(inner)?))),
            Type::Named { path, args, .. } => {
                let name = path.last().expect("chemin non vide").name.as_str();
                let arg = |i: usize| -> Result<Ty<'p>, String> {
                    args.get(i)
                        .map(|a| self.ty(a))
                        .unwrap_or_else(|| Err(format!("`{name}` attend un type en paramètre")))
                };
                Ok(match name {
                    "Int" => Ty::Int,
                    "Float" | "Money" => Ty::Float,
                    "String" | "Path" | "Email" | "Url" | "Symbol" => Ty::Str,
                    "Bool" => Ty::Bool,
                    "Unit" | "Nil" => Ty::Nil,
                    "Array" => Ty::Array(Box::new(arg(0)?)),
                    "Hash" => Ty::Hash(Box::new(arg(1)?)),
                    _ => match self.types.get(name) {
                        Some(info) => Ty::User(info.def.name.name.as_str()),
                        None => return Err(format!("type `{name}` inconnu")),
                    },
                })
            }
        }
    }

    fn schema(&self, ty: &Ty<'p>, depth: usize) -> Result<Json, String> {
        if depth > MAX_SCHEMA_DEPTH {
            return Err("type récursif : non représentable en JSON Schema".into());
        }
        Ok(match ty {
            Ty::Int => json!({"type": "integer"}),
            Ty::Float => json!({"type": "number"}),
            Ty::Str => json!({"type": "string"}),
            Ty::Bool => json!({"type": "boolean"}),
            Ty::Nil => json!({"type": "null"}),
            Ty::Array(item) => json!({"type": "array", "items": self.schema(item, depth + 1)?}),
            Ty::Opt(inner) => json!({"anyOf": [self.schema(inner, depth + 1)?, {"type": "null"}]}),
            Ty::Hash(_) => return Err("`Hash` ne peut pas sortir d'un LLM : utilisez une `struct`".into()),
            Ty::User(name) => {
                let info = &self.types[name];
                let mut schema = match info.def.kind {
                    TypeKind::Struct => {
                        let fields: Vec<_> =
                            info.fields.iter().map(|f| (*f, f.ty.as_ref(), f.default.is_some())).collect();
                        self.object_schema(
                            fields.into_iter().map(|(f, ty, opt)| (f.name.name.as_str(), ty, f.doc.as_deref(), opt)),
                            depth,
                        )?
                    }
                    TypeKind::Enum if info.variants.iter().all(|v| v.fields.is_empty()) => {
                        let names: Vec<_> = info.variants.iter().map(|v| v.name.name.as_str()).collect();
                        let mut s = json!({"type": "string", "enum": names});
                        let docs: Vec<String> = info
                            .variants
                            .iter()
                            .filter_map(|v| v.doc.as_ref().map(|d| format!("{} : {d}", v.name.name)))
                            .collect();
                        if !docs.is_empty() {
                            s["description"] = json!(docs.join("\n"));
                        }
                        s
                    }
                    TypeKind::Enum => {
                        let mut alternatives = Vec::new();
                        for variant in &info.variants {
                            let fields = variant
                                .fields
                                .iter()
                                .map(|f| (f.name.name.as_str(), f.ty.as_ref(), f.doc.as_deref(), f.default.is_some()));
                            let mut object = self.object_schema(fields, depth)?;
                            object["properties"]["kind"] = json!({"type": "string", "enum": [variant.name.name]});
                            object["required"].as_array_mut().expect("tableau").insert(0, json!("kind"));
                            alternatives.push(object);
                        }
                        json!({"anyOf": alternatives})
                    }
                    kind => return Err(format!("un {kind:?} (`{name}`) ne peut pas sortir d'un LLM")),
                };
                if let Some(doc) = &info.def.doc {
                    schema["description"] = json!(doc);
                }
                schema
            }
        })
    }

    /// Objet strict : tous les champs requis, ceux qui ont une valeur par défaut acceptent `null`.
    fn object_schema<'a>(
        &self,
        fields: impl Iterator<Item = (&'a str, Option<&'p Type>, Option<&'a str>, bool)>,
        depth: usize,
    ) -> Result<Json, String> {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for (name, ty, doc, has_default) in fields {
            let ty = ty.ok_or_else(|| format!("le champ `{name}` n'a pas de type"))?;
            let mut schema = self.schema(&self.ty(ty)?, depth + 1)?;
            if has_default {
                schema = json!({"anyOf": [schema, {"type": "null"}]});
            }
            if let Some(doc) = doc {
                schema["description"] = json!(doc);
            }
            properties.insert(name.to_string(), schema);
            required.push(json!(name));
        }
        Ok(json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}))
    }

    /// Schéma de sortie : un objet, en enveloppant `{"value": …}` si nécessaire.
    fn output_schema(&self, ty: &Ty<'p>) -> Result<(Json, bool), String> {
        let schema = self.schema(ty, 0)?;
        if schema["type"] == "object" {
            return Ok((schema, false));
        }
        let wrapped = json!({
            "type": "object",
            "properties": {"value": schema},
            "required": ["value"],
            "additionalProperties": false,
        });
        Ok((wrapped, true))
    }

    // ── JSON ↔ valeurs ───────────────────────────────────────

    pub(crate) fn json_to_value(&mut self, json: &Json, ty: &Ty<'p>) -> Result<Value<'p>, String> {
        let mismatch = |expected: &str| Err(format!("{expected}, reçu {json}"));
        match ty {
            Ty::Int => match json.as_i64().or_else(|| json.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)) {
                Some(n) => Ok(Value::Int(n)),
                None => mismatch("entier attendu"),
            },
            Ty::Float => json.as_f64().map(Value::Float).map_or_else(|| mismatch("nombre attendu"), Ok),
            Ty::Str => json.as_str().map(Value::str).map_or_else(|| mismatch("chaîne attendue"), Ok),
            Ty::Bool => json.as_bool().map(Value::Bool).map_or_else(|| mismatch("booléen attendu"), Ok),
            Ty::Nil => Ok(Value::Nil),
            Ty::Opt(inner) => {
                if json.is_null() {
                    Ok(Value::Nil)
                } else {
                    self.json_to_value(json, inner)
                }
            }
            Ty::Array(item) => {
                let Some(items) = json.as_array() else { return mismatch("tableau attendu") };
                let values = items.iter().map(|i| self.json_to_value(i, item)).collect::<Result<_, _>>()?;
                Ok(Value::array(values))
            }
            Ty::Hash(value_ty) => {
                let Some(object) = json.as_object() else { return mismatch("objet attendu") };
                let mut pairs = Vec::new();
                for (k, v) in object {
                    pairs.push((Value::str(k), self.json_to_value(v, value_ty)?));
                }
                Ok(Value::Hash(Rc::new(std::cell::RefCell::new(pairs))))
            }
            Ty::User(name) => self.json_to_user(json, name),
        }
    }

    fn json_to_user(&mut self, json: &Json, name: &'p str) -> Result<Value<'p>, String> {
        let info = &self.types[name];
        match info.def.kind {
            TypeKind::Struct => {
                let defs = info.fields.clone();
                let object = json.as_object().ok_or_else(|| format!("objet `{name}` attendu, reçu {json}"))?;
                let fields = self.json_fields(object, &defs, name)?;
                Ok(Value::record(name, fields))
            }
            TypeKind::Enum => {
                let variants = info.variants.clone();
                let (variant_name, object) = match json {
                    Json::String(s) => (s.as_str(), None),
                    Json::Object(o) => (o.get("kind").and_then(Json::as_str).unwrap_or_default(), Some(o)),
                    _ => return Err(format!("variante de `{name}` attendue, reçu {json}")),
                };
                let variant = variants
                    .iter()
                    .find(|v| v.name.name == variant_name)
                    .or_else(|| variants.iter().find(|v| v.name.name.eq_ignore_ascii_case(variant_name)))
                    .ok_or_else(|| format!("`{variant_name}` n'est pas une variante de `{name}`"))?;
                let defs: Vec<_> = variant.fields.iter().collect();
                let fields = match object {
                    Some(o) if !defs.is_empty() => self.json_fields(o, &defs, &variant.name.name)?,
                    _ => Vec::new(),
                };
                Ok(Value::Variant(Rc::new(Variant {
                    enum_name: name.into(),
                    name: variant.name.name.as_str().into(),
                    fields,
                })))
            }
            kind => Err(format!("un {kind:?} ne peut pas être construit depuis du JSON")),
        }
    }

    fn json_fields(
        &mut self,
        object: &Map<String, Json>,
        defs: &[&'p grenat_ast::Field],
        owner: &str,
    ) -> Result<Fields<'p>, String> {
        let mut fields = Vec::with_capacity(defs.len());
        for def in defs {
            let ty = def.ty.as_ref().ok_or_else(|| format!("champ `{}` sans type", def.name.name))?;
            let ty = self.ty(ty)?;
            let value = match object.get(&def.name.name) {
                None | Some(Json::Null) if def.default.is_some() => {
                    let default = def.default.as_ref().expect("vérifié");
                    self.eval(default).map_err(|_| format!("valeur par défaut de `{}` invalide", def.name.name))?
                }
                None if matches!(ty, Ty::Opt(_)) => Value::Nil,
                None => return Err(format!("champ `{}` manquant pour `{owner}`", def.name.name)),
                Some(json) => {
                    self.json_to_value(json, &ty).map_err(|e| format!("`{owner}.{}` : {e}", def.name.name))?
                }
            };
            fields.push((def.name.name.as_str().into(), value));
        }
        Ok(fields)
    }

    // ── Modèles et appels ────────────────────────────────────

    fn model(&mut self, selector: Option<&'p Expr>) -> Result<ModelConfig, Ctrl<'p>> {
        let Some(selector) = selector else {
            return match self.models.first() {
                Some((_, config)) => Ok(config.clone()),
                None => raise(
                    "LlmError",
                    "aucun modèle déclaré : ajoutez `model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"`",
                ),
            };
        };
        match self.eval(selector)? {
            Value::Symbol(name) => match self.models.iter().find(|(n, _)| **n == *name) {
                Some((_, config)) => Ok(config.clone()),
                None => raise("NameError", format!("modèle `:{name}` non déclaré")),
            },
            Value::Str(name) => Ok(ModelConfig::new("anthropic", &*name)),
            other => raise("TypeError", format!("modèle attendu (`:fast`), reçu {}", other.inspect())),
        }
    }

    fn provider(&mut self, model: &ModelConfig) -> Result<Rc<dyn grenat_llm::Provider>, Ctrl<'p>> {
        if let Some(provider) = &self.provider {
            return Ok(provider.clone());
        }
        if model.provider != "anthropic" {
            return raise(
                "LlmError",
                format!("fournisseur `:{}` non pris en charge (phase 1 : `:anthropic`)", model.provider),
            );
        }
        match Anthropic::from_env() {
            Ok(client) => {
                let provider: Rc<dyn grenat_llm::Provider> = Rc::new(client);
                self.provider = Some(provider.clone());
                Ok(provider)
            }
            Err(e) => raise("LlmError", e.message),
        }
    }

    fn llm_call(&mut self, request: &Request) -> Result<Response, Ctrl<'p>> {
        self.check_budgets()?;
        let provider = self.provider(request.model)?;
        let started = Instant::now();
        let response = match provider.complete(request) {
            Ok(r) => r,
            Err(e) => return raise("LlmError", e.message),
        };
        self.llm_calls += 1;
        // après un repli côté serveur, c'est le modèle qui a répondu qui est facturé
        let cost = cost_usd(&response.model, &response.usage)
            .or_else(|| cost_usd(&request.model.name, &response.usage))
            .unwrap_or(0.0);
        for budget in &self.budgets {
            budget.spent_usd.set(budget.spent_usd.get() + cost);
            budget.tokens.set(budget.tokens.get() + response.usage.total_tokens());
        }
        if self.log {
            let line = format!(
                "[llm] {} · {} in / {} out · {} · {:.1}s\n",
                request.model.name,
                response.usage.input_tokens,
                response.usage.output_tokens,
                money(cost),
                started.elapsed().as_secs_f64()
            );
            self.write_err(&line);
        }
        match response.stop_reason.as_str() {
            "refusal" => return raise("LlmRefusal", "le modèle a refusé la requête"),
            "max_tokens" => return raise("LlmError", "réponse tronquée : augmentez `max_tokens` du modèle"),
            _ => {}
        }
        self.check_budgets()?;
        Ok(response)
    }

    // ── prompt ───────────────────────────────────────────────

    /// Corps d'un `prompt` : ses `system`/`user` construisent la requête.
    /// Appelé par `call_fn`, paramètres déjà liés.
    pub(crate) fn run_prompt(&mut self, def: &'p FnDef) -> R<'p> {
        self.prompts.push(crate::PromptCtx::default());
        let body = self.eval_body(&def.body);
        let ctx = self.prompts.pop().expect("contexte de prompt");
        body?;

        let ret = match &def.ret {
            Some(t) => self.ty(t).or_else(type_error)?,
            None => Ty::Str,
        };
        let model = self.model(def.model.as_ref())?;
        let system: Vec<String> = def.doc.iter().cloned().chain(ctx.system).collect();
        if !ctx.messages.iter().any(|(role, _)| *role == "user") {
            return raise(
                "LlmError",
                format!("le prompt `{}` n'envoie aucun message : ajoutez `user \"…\"`", def.name.name),
            );
        }
        let mut messages: Vec<Json> = Vec::new();
        for (role, text) in ctx.messages {
            match messages.last_mut() {
                Some(last) if last["role"] == role => {
                    let merged = format!("{}\n\n{text}", last["content"].as_str().unwrap_or_default());
                    last["content"] = json!(merged);
                }
                _ => messages.push(json!({"role": role, "content": text})),
            }
        }
        let output = match ret {
            Ty::Str => None,
            ref other => Some(self.output_schema(other).or_else(type_error)?),
        };
        let request = Request {
            model: &model,
            system: (!system.is_empty()).then(|| system.join("\n\n")),
            messages,
            tools: Vec::new(),
            output_schema: output.as_ref().map(|(schema, _)| schema.clone()),
        };

        let mut last_error = String::new();
        for _attempt in 0..2 {
            let response = self.llm_call(&request)?;
            let text = response.text();
            let Some((_, wrapped)) = &output else {
                return Ok(Value::str(text).taint());
            };
            let parsed = serde_json::from_str::<Json>(text.trim()).map_err(|e| format!("JSON invalide : {e}"));
            let converted = parsed.and_then(|json| {
                let payload = if *wrapped { json["value"].clone() } else { json };
                self.json_to_value(&payload, &ret)
            });
            match converted {
                Ok(value) => return Ok(value.taint()),
                Err(e) => last_error = e,
            }
        }
        raise("LlmError", format!("sortie invalide pour `{}` : {last_error}", def.name.name))
    }

    // ── Agents : boucle `run` ────────────────────────────────

    fn agent_config(&mut self, ty: &str) -> Result<AgentConfig<'p>, Ctrl<'p>> {
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

    fn tool_spec(&self, def: &'p FnDef) -> Result<ToolSpec, String> {
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
    fn call_tool(&mut self, tool_use: &ToolUse) -> R<'p> {
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

    // ── system / user dans un prompt ─────────────────────────

    pub(crate) fn prompt_message(&mut self, role: &'static str, text: String) -> bool {
        let Some(ctx) = self.prompts.last_mut() else { return false };
        if role == "system" {
            ctx.system.push(text);
        } else {
            ctx.messages.push((role, text));
        }
        true
    }
}

fn tool_result(id: &str, content: String, is_error: bool) -> Json {
    json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error})
}

fn tool_output(value: &Value) -> String {
    match value.untainted() {
        Value::Str(s) => s.to_string(),
        Value::Nil => "ok".into(),
        other => value_to_json(other).to_string(),
    }
}

/// Représentation JSON d'une valeur (résultats d'outils, `Json.dump`).
pub(crate) fn value_to_json(value: &Value) -> Json {
    match value.untainted() {
        Value::Nil => Json::Null,
        Value::Bool(b) => json!(b),
        Value::Int(n) => json!(n),
        Value::Float(f) | Value::Money(f) | Value::Duration(f) => json!(f),
        Value::Str(s) | Value::Symbol(s) | Value::Type(s) => json!(&**s),
        Value::Array(items) => Json::Array(items.borrow().iter().map(value_to_json).collect()),
        Value::Hash(entries) => {
            Json::Object(entries.borrow().iter().map(|(k, v)| (k.to_display(), value_to_json(v))).collect())
        }
        Value::Range(lo, hi, inclusive) => {
            let hi = if *inclusive { *hi } else { hi - 1 };
            Json::Array((*lo..=hi).map(|n| json!(n)).collect())
        }
        Value::Record(r) => fields_json(&r.fields),
        Value::Variant(v) if v.fields.is_empty() => json!(&*v.name),
        Value::Variant(v) => {
            let mut object = fields_json(&v.fields);
            object["kind"] = json!(&*v.name);
            object
        }
        Value::Error(e) => json!({"error": &*e.ty, "message": e.message}),
        other => json!(other.inspect()),
    }
}

fn fields_json(fields: &Fields) -> Json {
    Json::Object(fields.iter().map(|(k, v)| (k.to_string(), value_to_json(v))).collect())
}
