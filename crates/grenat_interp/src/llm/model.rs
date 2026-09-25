//! Model resolution, LLM provider and accounted calls (budgets, logging).

use crate::prelude::*;
use grenat_llm::{Anthropic, ModelConfig, Request, Response, cost_usd};

use crate::value::money;

impl<'p> Interp<'p> {
    // ── Models and calls ────────────────────────────────────

    pub(crate) fn model(&mut self, selector: Option<&'p Expr>) -> Result<ModelConfig, Ctrl<'p>> {
        let Some(selector) = selector else {
            let first = self.models.borrow().first().map(|(_, c)| c.clone());
            return match first {
                Some(config) => Ok(config),
                None => raise(
                    "LlmError",
                    "no model declared: add `model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"`",
                ),
            };
        };
        match self.eval(selector)? {
            Value::Symbol(name) => self.model_named(&name),
            Value::Str(name) => Ok(ModelConfig::new("anthropic", &*name)),
            other => raise("TypeError", format!("expected a model (`:fast`), got {}", other.inspect())),
        }
    }

    /// The model declared as `:name`.
    pub(crate) fn model_named(&self, name: &str) -> Result<ModelConfig, Ctrl<'p>> {
        match self.models.borrow().iter().find(|(n, _)| n == name).map(|(_, c)| c.clone()) {
            Some(config) => Ok(config),
            None => raise("NameError", format!("model `:{name}` is not declared")),
        }
    }

    /// Who answers for `model`: its mock, else the enclosing cassette, else
    /// the forced provider, else the real one (never in offline runs).
    pub(crate) fn provider(&mut self, model: &ModelConfig) -> Result<Arc<dyn grenat_llm::Provider>, Ctrl<'p>> {
        if let Some(mock) = self.mock_for(model) {
            return Ok(mock);
        }
        if let Some(provider) = self.providers.last() {
            return Ok(provider.clone());
        }
        self.real_provider(model)
    }

    /// The provider outside any mock or cassette.
    pub(crate) fn real_provider(&mut self, model: &ModelConfig) -> Result<Arc<dyn grenat_llm::Provider>, Ctrl<'p>> {
        if let Some(provider) = self.provider.borrow().clone() {
            return Ok(provider);
        }
        if self.offline {
            return raise(
                "LlmError",
                format!(
                    "no real model in tests: `{}` is called outside any `mock` or `cassette \"…\" do … end`",
                    model.name
                ),
            );
        }
        if model.provider != "anthropic" {
            return raise("LlmError", format!("unsupported provider `:{}` (available: `:anthropic`)", model.provider));
        }
        match Anthropic::from_env() {
            Ok(client) => {
                let provider: Arc<dyn grenat_llm::Provider> = Arc::new(client);
                // several tasks may create the client at the same time: the first one wins
                let provider = self.provider.borrow_mut().get_or_insert(provider).clone();
                Ok(provider)
            }
            Err(e) => raise("LlmError", e.message),
        }
    }

    pub(crate) fn llm_call(&mut self, request: &Request) -> Result<Response, Ctrl<'p>> {
        self.check_cancel()?;
        self.check_budgets()?;
        let started = Instant::now();
        let batched = self.batch.clone();
        let answer = match &batched {
            // queued for the batch; this task waits for its round
            Some(batch) => batch.call(request),
            None => {
                let provider = self.provider(request.model)?;
                // off the worker threads: the other tasks run meanwhile
                grenat_green::blocking(|| provider.complete(request))
            }
        };
        let response = match answer {
            Ok(r) => r,
            Err(e) => return raise("LlmError", e.message),
        };
        self.llm_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // after a server-side fallback, the model that actually answered is billed
        let (billed, cost) = match cost_usd(&response.model, &response.usage) {
            Some(cost) => (response.model.clone(), cost),
            None => (request.model.name.clone(), cost_usd(&request.model.name, &response.usage).unwrap_or(0.0)),
        };
        // a batch costs half
        let cost = if batched.is_some() { cost / 2.0 } else { cost };
        for budget in &self.budgets {
            budget.add(cost, response.usage.total_tokens());
        }
        self.record_call(&billed, &response.usage, cost);
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
            "refusal" => return raise("LlmRefusal", "the model refused the request"),
            "max_tokens" => return raise("LlmError", "truncated response: increase the model's `max_tokens`"),
            _ => {}
        }
        self.check_budgets()?;
        Ok(response)
    }
}
