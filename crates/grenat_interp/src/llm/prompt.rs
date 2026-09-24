//! Running `prompt`s: building the request, structured output, validation.

use crate::prelude::*;
use grenat_llm::Request;
use serde_json::{Value as Json, json};

use super::*;

impl<'p> Interp<'p> {
    // ── prompt ───────────────────────────────────────────────

    /// Body of a `prompt`: its `system`/`user` calls build the request.
    /// Called by `call_fn`, with parameters already bound.
    pub(crate) fn run_prompt(&mut self, def: &'p FnDef) -> R<'p> {
        self.prompts.push(crate::PromptCtx::default());
        let body = self.eval_body(&def.body);
        let ctx = self.prompts.pop().expect("prompt context");
        body?;

        let ret = match &def.ret {
            Some(t) => self.ty(t).or_else(type_error)?,
            None => Ty::Str,
        };
        let model = self.model(def.model.as_ref())?;
        let system: Vec<String> = def.doc.iter().cloned().chain(ctx.system).collect();
        if !ctx.messages.iter().any(|(role, _)| *role == "user") {
            return raise("LlmError", format!("prompt `{}` sends no message: add `user \"…\"`", def.name.name));
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
            let parsed = serde_json::from_str::<Json>(text.trim()).map_err(|e| format!("invalid JSON: {e}"));
            let converted = parsed.and_then(|json| {
                let payload = if *wrapped { json["value"].clone() } else { json };
                self.json_to_value(&payload, &ret)
            });
            match converted {
                Ok(value) => return Ok(value.taint()),
                Err(e) => last_error = e,
            }
        }
        raise("LlmError", format!("invalid output for `{}`: {last_error}", def.name.name))
    }

    // ── system / user inside a prompt ─────────────────────────

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
