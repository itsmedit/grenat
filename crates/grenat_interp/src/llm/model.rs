//! Model resolution, LLM provider and accounted calls (budgets, logging).

use crate::prelude::*;
use grenat_llm::catalog::{self, Protocol};
use grenat_llm::{Anthropic, ModelConfig, OpenAi, Request, Response, cost_at, cost_usd};

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

    /// The cost of a call to a model whose price is unknown: nothing, said once.
    fn price_or_warn(&mut self, model: &ModelConfig, usage: &grenat_llm::Usage) -> f64 {
        if let Some(cost) = cost_usd(&model.name, usage) {
            return cost;
        }
        if self.unpriced.borrow_mut().insert(model.name.clone()) {
            self.write_err(&format!(
                "warning: the price of `{}` is unknown: budgets in dollars do not count it (give `price: {{input: …, output: …}}`, dollars per million tokens, in config/models.yml)\n",
                model.name
            ));
        }
        0.0
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
        let Some(catalogued) = catalog::provider(&model.provider) else {
            return raise("LlmError", format!("unknown provider `{}` (known: {})", model.provider, catalog::names()));
        };
        let base_url = model.base_url.clone().or_else(|| {
            // the Messages API's own override
            (catalogued.protocol == Protocol::Anthropic).then(|| std::env::var("ANTHROPIC_BASE_URL").ok()).flatten()
        });
        let key = format!("{}|{}", catalogued.name, base_url.as_deref().unwrap_or(catalogued.base_url));
        if let Some(client) = self.clients.borrow().get(&key) {
            return Ok(client.clone());
        }
        let api_key = self.api_key(catalogued)?;
        let client: Arc<dyn grenat_llm::Provider> = match catalogued.protocol {
            Protocol::Anthropic => {
                Arc::new(Anthropic::new(api_key.unwrap_or_default(), base_url.as_deref().unwrap_or(catalogued.base_url)))
            }
            Protocol::OpenAi | Protocol::Responses => Arc::new(OpenAi::new(*catalogued, api_key, base_url.as_deref())),
        };
        // several tasks may create the client at the same time: the first one wins
        Ok(self.clients.borrow_mut().entry(key).or_insert(client).clone())
    }

    /// A provider's key: `<provider>.api_key` in the credentials, else its
    /// environment variable (`OPENAI_API_KEY`…).
    fn api_key(&mut self, provider: &catalog::Catalogued) -> Result<Option<String>, Ctrl<'p>> {
        let credentials = self.credentials_tree();
        let from_credentials = credentials.as_ref().ok().and_then(|c| c[provider.name]["api_key"].as_str()).map(String::from);
        let key = from_credentials.or_else(|| std::env::var(provider.key_variable).ok()).filter(|k| !k.is_empty());
        if key.is_none() && provider.key_required {
            let mut message = format!(
                "no key for `{}`: add `{}: api_key: …` to the credentials (grenat credentials edit), or set {}",
                provider.name, provider.name, provider.key_variable
            );
            if let Err(Ctrl::Raise(e)) = &credentials
                && !e.message.starts_with("no credentials")
            {
                message.push_str(&format!(" ({})", e.message));
            }
            return raise("LlmError", message);
        }
        Ok(key)
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
        // the price given for the model; else, after a server-side fallback,
        // the model that actually answered is billed
        let (billed, cost) = match (request.model.price, cost_usd(&response.model, &response.usage)) {
            (Some(price), _) => (request.model.name.clone(), cost_at(price, &response.usage)),
            (None, Some(cost)) => (response.model.clone(), cost),
            (None, None) => (request.model.name.clone(), self.price_or_warn(request.model, &response.usage)),
        };
        // a batch costs half, where the provider has a batch API
        let discounted = catalog::provider(&request.model.provider).is_some_and(|p| p.protocol == Protocol::Anthropic);
        let cost = if batched.is_some() && discounted { cost / 2.0 } else { cost };
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
