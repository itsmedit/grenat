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
            Value::Symbol(name) => {
                match self.models.borrow().iter().find(|(n, _)| **n == *name).map(|(_, c)| c.clone()) {
                    Some(config) => Ok(config),
                    None => raise("NameError", format!("model `:{name}` is not declared")),
                }
            }
            Value::Str(name) => Ok(ModelConfig::new("anthropic", &*name)),
            other => raise("TypeError", format!("expected a model (`:fast`), got {}", other.inspect())),
        }
    }

    pub(crate) fn provider(&mut self, model: &ModelConfig) -> Result<Arc<dyn grenat_llm::Provider>, Ctrl<'p>> {
        if let Some(provider) = self.provider.borrow().clone() {
            return Ok(provider);
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
        let provider = self.provider(request.model)?;
        let started = Instant::now();
        // off the worker threads: the other tasks run meanwhile
        let response = match grenat_green::blocking(|| provider.complete(request)) {
            Ok(r) => r,
            Err(e) => return raise("LlmError", e.message),
        };
        self.llm_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // after a server-side fallback, the model that actually answered is billed
        let cost = cost_usd(&response.model, &response.usage)
            .or_else(|| cost_usd(&request.model.name, &response.usage))
            .unwrap_or(0.0);
        for budget in &self.budgets {
            budget.add(cost, response.usage.total_tokens());
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
            "refusal" => return raise("LlmRefusal", "the model refused the request"),
            "max_tokens" => return raise("LlmError", "truncated response: increase the model's `max_tokens`"),
            _ => {}
        }
        self.check_budgets()?;
        Ok(response)
    }
}
