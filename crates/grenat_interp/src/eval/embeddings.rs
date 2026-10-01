//! Embeddings: `embed(:docs, text)` is the vector of a text (an
//! `Array(Float)`), `embed(:docs, texts)` those of several texts, sent in as
//! few requests as the provider accepts. The model is declared with
//! `kind: :embedding`; the call is an `llm` effect, counted by budgets and
//! recorded with its cost (input tokens only, at `price: {input: …}`).
//!
//! A vector is numbers, never instructions: it is not untrusted, even when
//! its text is. In tests, `mock_embed :docs` makes vectors from the texts'
//! words (or gives them: `vectors: {"refund" => [1.0, 0.0]}`).

use std::collections::HashMap;

use grenat_llm::{EmbeddingRequest, FakeEmbeddings, ModelConfig, ModelKind, Provider, catalog, cost_at};

use crate::builtins::number;
use crate::prelude::*;
use crate::value::money;

/// The size of fake vectors when nothing gives it.
const FAKE_DIMENSIONS: usize = 256;

impl<'p> Interp<'p> {
    /// `embed(:docs, text)` or `embed(:docs, texts)`; without a model, the
    /// first embedding model declared.
    pub(crate) fn embed(&mut self, model: Option<&str>, input: &Value<'p>) -> R<'p> {
        self.check_effect("llm")?;
        crate::eval::secrets::not_for_models(std::slice::from_ref(input))?;
        let model = self.embedding_model(model)?;
        let (texts, many) = match input.untainted() {
            Value::Str(text) => (vec![text.to_string()], false),
            Value::Array(items) => {
                let mut texts = Vec::new();
                for item in items.borrow().iter() {
                    match item.untainted() {
                        Value::Str(text) => texts.push(text.to_string()),
                        other => {
                            return raise("TypeError", format!("`embed` expects texts, got {}", other.inspect()));
                        }
                    }
                }
                (texts, true)
            }
            other => {
                return raise(
                    "TypeError",
                    format!("`embed` expects a text or an array of texts, got {}", other.inspect()),
                );
            }
        };
        if texts.iter().any(|t| t.trim().is_empty()) {
            return raise("ArgumentError", "`embed`: an empty text has no meaning to embed");
        }
        if texts.is_empty() {
            return Ok(Value::array(Vec::new()));
        }
        let vectors = self.embedding_call(&model, texts)?;
        let mut values: Vec<Value<'p>> =
            vectors.into_iter().map(|v| Value::array(v.into_iter().map(Value::Float).collect())).collect();
        Ok(if many { Value::array(values) } else { values.remove(0) })
    }

    /// The model `name` names, which must make embeddings; else the first that does.
    fn embedding_model(&self, name: Option<&str>) -> Result<ModelConfig, Ctrl<'p>> {
        let Some(name) = name else {
            let first =
                self.models.borrow().iter().find(|(_, c)| c.kind == ModelKind::Embedding).map(|(_, c)| c.clone());
            return first.map_or_else(
                || {
                    raise(
                        "LlmError",
                        "no embedding model declared: add `model :docs, provider: :openai, name: \"text-embedding-3-small\", kind: :embedding`",
                    )
                },
                Ok,
            );
        };
        let model = self.model_named(name)?;
        if model.kind != ModelKind::Embedding {
            return raise(
                "LlmError",
                format!("`:{name}` is not an embedding model: declare one with `kind: :embedding`"),
            );
        }
        Ok(model)
    }

    /// The vectors of `texts`, accounted: budgets, the ledger, the log.
    fn embedding_call(&mut self, model: &ModelConfig, texts: Vec<String>) -> Result<Vec<Vec<f64>>, Ctrl<'p>> {
        self.check_cancel()?;
        self.check_budgets()?;
        let started = Instant::now();
        let provider = self.embedder(model)?;
        let count = texts.len();
        let request = EmbeddingRequest { model, inputs: texts };
        let answer = match grenat_green::blocking(|| provider.embed(&request)) {
            Ok(answer) => answer,
            Err(e) => return raise("LlmError", e.message),
        };
        if answer.vectors.len() != count {
            return raise("LlmError", format!("{count} texts embedded, {} vectors received", answer.vectors.len()));
        }
        if let Some(size) = model.dimensions
            && let Some(other) = answer.vectors.iter().find(|v| v.len() != size as usize)
        {
            return raise(
                "LlmError",
                format!("`{}` gave a vector of {} dimensions, not the {size} declared", model.name, other.len()),
            );
        }
        self.llm_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cost = self.embedding_cost(model, &answer.usage);
        for budget in &self.budgets {
            budget.add(cost, answer.usage.total_tokens());
        }
        self.record_call(&model.name, &answer.usage, cost);
        if self.log {
            let line = format!(
                "[embed] {} · {count} text(s) · {} in · {} · {:.1}s\n",
                model.name,
                answer.usage.input_tokens,
                money(cost),
                started.elapsed().as_secs_f64()
            );
            self.write_err(&line);
        }
        self.check_budgets()?;
        Ok(answer.vectors)
    }

    /// At the declared price; an unknown one counts nothing, said once
    /// (a fake's vectors cost no token, hence nothing to say).
    fn embedding_cost(&mut self, model: &ModelConfig, usage: &grenat_llm::Usage) -> f64 {
        if let Some(price) = model.price {
            return cost_at(price, usage);
        }
        if usage.input_tokens > 0 && self.unpriced.borrow_mut().insert(model.name.clone()) {
            self.write_err(&format!(
                "warning: the price of `{}` is unknown: budgets in dollars do not count it (give `price: {{input: …}}`, dollars per million tokens, in config/models.yml)\n",
                model.name
            ));
        }
        0.0
    }

    /// Who makes the vectors: the model's fake, else the forced provider,
    /// else the real one (never in offline runs).
    fn embedder(&mut self, model: &ModelConfig) -> Result<Arc<dyn Provider>, Ctrl<'p>> {
        let fake = {
            let fakes = self.embedding_mocks.borrow();
            let own = fakes.iter().find(|(m, _)| m.as_ref() == Some(model));
            own.or_else(|| fakes.iter().find(|(m, _)| m.is_none())).map(|(_, f)| f.clone())
        };
        if let Some(fake) = fake {
            return Ok(fake);
        }
        if self.provider.borrow().is_none() && self.offline {
            return raise("LlmError", format!("no real model in tests: `{}` embeds outside `mock_embed`", model.name));
        }
        self.real_provider(model)
    }

    /// `mock_embed :docs` (or any embedding model, without a name): vectors
    /// made from the texts' words, or given (`vectors: {"text" => [...]}`),
    /// of `dimensions:` floats (else the given vectors', else the model's).
    pub(crate) fn mock_embed(&mut self, model: Option<&str>, args: &Args<'p>) -> R<'p> {
        let config = match model {
            Some(name) => Some(self.embedding_model(Some(name))?),
            None => None,
        };
        let mut known = HashMap::new();
        let mut dimensions = None;
        for (option, value) in &args.named {
            match (option.as_str(), value.untainted()) {
                ("dimensions", Value::Int(n)) if *n > 0 => dimensions = Some(*n as usize),
                ("vectors", Value::Hash(pairs)) => {
                    for (text, vector) in pairs.borrow().iter() {
                        known.insert(text.to_display(), floats(vector)?);
                    }
                }
                (option, v) => {
                    return raise("ArgumentError", format!("invalid `mock_embed` option `{option}: {}`", v.inspect()));
                }
            }
        }
        let sizes: std::collections::BTreeSet<usize> = known.values().map(Vec::len).collect();
        if sizes.len() > 1 {
            let sizes: Vec<String> = sizes.iter().map(usize::to_string).collect();
            return raise(
                "ArgumentError",
                format!("`mock_embed`: the vectors given have different sizes ({})", sizes.join(", ")),
            );
        }
        let given = sizes.first().copied();
        if let (Some(given), Some(size)) = (given, dimensions)
            && given != size
        {
            return raise(
                "ArgumentError",
                format!("`mock_embed`: vectors of {given} floats are given, not of the {size} of `dimensions:`"),
            );
        }
        let size = dimensions.or(given).unwrap_or_else(|| {
            let declared = config.as_ref().and_then(|c| c.dimensions.or_else(|| catalog::default_dimensions(&c.name)));
            declared.map_or(FAKE_DIMENSIONS, |n| n as usize)
        });
        let label = model.map_or_else(|| "every embedding model".to_string(), |name| format!("`:{name}`"));
        let fake = Arc::new(FakeEmbeddings::new(label, size, known));
        // the latest fake of a model replaces the previous one
        let mut fakes = self.embedding_mocks.borrow_mut();
        fakes.retain(|(m, _)| *m != config);
        fakes.push((config, fake));
        Ok(Value::Nil)
    }
}

/// The floats of an array of numbers (a vector given to `mock_embed`,
/// stored, searched): finite, and within the range of the 32-bit floats a
/// database keeps — a larger one would be kept as infinity.
pub(crate) fn floats<'p>(value: &Value<'p>) -> Result<Vec<f64>, Ctrl<'p>> {
    let Value::Array(items) = value.untainted() else {
        return raise("TypeError", format!("a vector is an array of numbers, got {}", value.inspect()));
    };
    items
        .borrow()
        .iter()
        .map(|x| match number(x) {
            Some(f) if f.is_finite() && f.abs() <= f64::from(f32::MAX) => Ok(f),
            Some(f) => raise("TypeError", format!("a vector holds 32-bit floats: {f:e} is out of their range")),
            None => raise("TypeError", format!("a vector holds numbers, got {}", x.inspect())),
        })
        .collect()
}
