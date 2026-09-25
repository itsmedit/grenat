//! `judge(:model, question, context…)`: a model grades an output, from 0 to 1
//! (LLM-as-judge, for evals). The score comes through structured output and
//! is validated, hence untainted.

use crate::prelude::*;
use grenat_llm::Request;
use serde_json::{Value as Json, json};

const SYSTEM: &str = "You are a strict, impartial evaluator. Read the question and the material, \
reason briefly, then give a score from 0 (not at all) to 1 (fully).";

impl<'p> Interp<'p> {
    pub(crate) fn judge(&mut self, model: Option<&str>, question: &str, context: &[Value<'p>]) -> R<'p> {
        let model = match model {
            Some(name) => self.model_named(name)?,
            None => self.model(None)?,
        };
        let mut content = format!("Question: {question}");
        for (i, item) in context.iter().enumerate() {
            content.push_str(&format!("\n\n<material index=\"{}\">\n{}\n</material>", i + 1, item.to_display()));
        }
        let request = Request {
            model: &model,
            system: Some(SYSTEM.into()),
            messages: vec![json!({"role": "user", "content": content})],
            tools: Vec::new(),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "reasoning": {"type": "string", "description": "One or two sentences, before the score."},
                    "score": {"type": "number", "description": "From 0 to 1."},
                },
                "required": ["reasoning", "score"],
                "additionalProperties": false,
            })),
        };
        let response = self.llm_call(&request)?;
        let score = serde_json::from_str::<Json>(response.text().trim()).ok().and_then(|j| j["score"].as_f64());
        match score {
            Some(score) if (0.0..=1.0).contains(&score) => Ok(Value::Float(score)),
            _ => raise("LlmError", format!("the judge gave no score from 0 to 1: {}", response.text())),
        }
    }
}
