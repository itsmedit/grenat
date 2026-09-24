//! Accès aux modèles de langage.
//!
//! Le runtime ne dépend que du trait [`Provider`] : l'implémentation réelle
//! ([`Anthropic`]) parle HTTP à l'API Messages, [`Scripted`] rejoue des
//! réponses préparées pour les tests.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use serde_json::{Value as Json, json};

/// Configuration d'un modèle, issue d'une déclaration `model :nom, …`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelConfig {
    pub provider: String,
    pub name: String,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
    /// `low` … `max` (`output_config.effort`).
    pub effort: Option<String>,
    /// Repli côté serveur quand le modèle refuse (`fallbacks: "default"`).
    pub fallbacks: bool,
}

impl ModelConfig {
    pub fn new(provider: impl Into<String>, name: impl Into<String>) -> Self {
        let name = name.into();
        // recommandé pour les modèles dont les classifieurs peuvent refuser une requête
        let fallbacks = matches!(name.as_str(), "claude-opus-5" | "claude-fable-5-1");
        ModelConfig { provider: provider.into(), name, max_tokens: 16_000, temperature: None, effort: None, fallbacks }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Json,
}

#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub model: &'a ModelConfig,
    pub system: Option<String>,
    /// Messages au format de l'API (`{"role": …, "content": …}`).
    pub messages: Vec<Json>,
    pub tools: Vec<ToolSpec>,
    /// Sortie structurée : JSON Schema de la réponse attendue.
    pub output_schema: Option<Json>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Json,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// Blocs de contenu bruts, à renvoyer tels quels dans l'historique.
    pub content: Vec<Json>,
    pub stop_reason: String,
    pub usage: Usage,
    pub model: String,
}

impl Response {
    /// Concaténation des blocs `text`.
    pub fn text(&self) -> String {
        self.content.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect()
    }

    pub fn tool_uses(&self) -> Vec<ToolUse> {
        self.content
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| ToolUse {
                id: b["id"].as_str().unwrap_or_default().to_string(),
                name: b["name"].as_str().unwrap_or_default().to_string(),
                input: b["input"].clone(),
            })
            .collect()
    }

    // ── Constructeurs pour les tests ──

    pub fn text_reply(text: impl Into<String>) -> Self {
        Response::from_content(vec![json!({"type": "text", "text": text.into()})], "end_turn")
    }

    pub fn json_reply(value: Json) -> Self {
        Response::text_reply(value.to_string())
    }

    pub fn tool_call(id: &str, name: &str, input: Json) -> Self {
        Response::from_content(vec![json!({"type": "tool_use", "id": id, "name": name, "input": input})], "tool_use")
    }

    pub fn from_content(content: Vec<Json>, stop_reason: &str) -> Self {
        Response {
            content,
            stop_reason: stop_reason.into(),
            usage: Usage { input_tokens: 100, output_tokens: 20, ..Usage::default() },
            model: "scripted".into(),
        }
    }

    pub fn with_usage(mut self, input_tokens: u64, output_tokens: u64) -> Self {
        self.usage = Usage { input_tokens, output_tokens, ..Usage::default() };
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmError {
    pub message: String,
}

impl LlmError {
    fn new(message: impl Into<String>) -> Self {
        LlmError { message: message.into() }
    }
}

pub trait Provider {
    fn complete(&self, request: &Request) -> Result<Response, LlmError>;
}

/// Prix en dollars par million de tokens (entrée, sortie).
fn price_per_mtok(model: &str) -> Option<(f64, f64)> {
    const PRICES: &[(&str, f64, f64)] = &[
        ("claude-fable-5", 10.0, 50.0),
        ("claude-mythos-5", 10.0, 50.0),
        ("claude-opus-5-5", 4.0, 20.0),
        ("claude-opus-5", 5.0, 25.0),
        ("claude-opus-4", 5.0, 25.0),
        ("claude-sonnet-5", 2.0, 10.0),
        ("claude-sonnet-4", 3.0, 15.0),
        ("claude-haiku-4-5", 1.0, 5.0),
    ];
    PRICES.iter().find(|(prefix, ..)| model.starts_with(prefix)).map(|&(_, input, output)| (input, output))
}

/// Coût d'un appel en dollars ; `None` pour un modèle dont le prix est inconnu.
pub fn cost_usd(model: &str, usage: &Usage) -> Option<f64> {
    let (input, output) = price_per_mtok(model)?;
    let input_equiv = usage.input_tokens as f64
        + usage.cache_creation_input_tokens as f64 * 1.25
        + usage.cache_read_input_tokens as f64 * 0.1;
    Some((input_equiv * input + usage.output_tokens as f64 * output) / 1_000_000.0)
}

/// Corps JSON d'une requête `POST /v1/messages`.
pub fn request_body(request: &Request) -> Json {
    let model = request.model;
    let mut body = json!({
        "model": model.name,
        "max_tokens": model.max_tokens,
        "messages": request.messages,
    });
    if let Some(system) = &request.system {
        body["system"] = json!(system);
    }
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema, "strict": true}))
            .collect();
    }
    if model.fallbacks {
        body["fallbacks"] = json!("default");
    }
    if let Some(temperature) = model.temperature {
        body["temperature"] = json!(temperature);
    }
    let mut output_config = serde_json::Map::new();
    if let Some(schema) = &request.output_schema {
        output_config.insert("format".into(), json!({"type": "json_schema", "schema": schema}));
    }
    if let Some(effort) = &model.effort {
        output_config.insert("effort".into(), json!(effort));
    }
    if !output_config.is_empty() {
        body["output_config"] = Json::Object(output_config);
    }
    body
}

fn parse_response(body: &Json) -> Result<Response, LlmError> {
    let content = body["content"].as_array().cloned().ok_or_else(|| LlmError::new("réponse sans `content`"))?;
    let usage = &body["usage"];
    let tokens = |field: &str| usage[field].as_u64().unwrap_or(0);
    Ok(Response {
        content,
        stop_reason: body["stop_reason"].as_str().unwrap_or_default().to_string(),
        usage: Usage {
            input_tokens: tokens("input_tokens"),
            output_tokens: tokens("output_tokens"),
            cache_creation_input_tokens: tokens("cache_creation_input_tokens"),
            cache_read_input_tokens: tokens("cache_read_input_tokens"),
        },
        model: body["model"].as_str().unwrap_or_default().to_string(),
    })
}

/// Client HTTP de l'API Messages d'Anthropic.
pub struct Anthropic {
    agent: ureq::Agent,
    api_key: String,
    base_url: String,
}

const MAX_ATTEMPTS: u32 = 4;

impl Anthropic {
    /// Lit `ANTHROPIC_API_KEY` (et `ANTHROPIC_BASE_URL`, optionnelle).
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| LlmError::new("ANTHROPIC_API_KEY n'est pas définie (export ANTHROPIC_API_KEY=sk-ant-…)"))?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into());
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(600)))
            .build()
            .into();
        Ok(Anthropic { agent, api_key, base_url: base_url.trim_end_matches('/').to_string() })
    }

    fn send(&self, body: &Json) -> Result<(u16, Option<u64>, Json), String> {
        let mut request = self
            .agent
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");
        if body.get("fallbacks").is_some() {
            request = request.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        let mut response = request.send_json(body).map_err(|e| format!("connexion impossible : {e}"))?;
        let status = response.status().as_u16();
        let retry_after =
            response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok());
        let json = response.body_mut().read_json::<Json>().map_err(|e| format!("réponse illisible : {e}"))?;
        Ok((status, retry_after, json))
    }
}

impl Provider for Anthropic {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        let body = request_body(request);
        let mut last_error = String::new();
        for attempt in 0..MAX_ATTEMPTS {
            let wait = match self.send(&body) {
                Ok((200, _, json)) => return parse_response(&json),
                Ok((status, retry_after, json)) => {
                    let message = json["error"]["message"].as_str().unwrap_or("erreur inconnue");
                    last_error = format!("HTTP {status} : {message}");
                    // 408, 409, 429, 5xx (dont 529 « overloaded ») : on réessaie
                    if !(matches!(status, 408 | 409 | 429) || status >= 500) {
                        break;
                    }
                    retry_after.unwrap_or(1 << attempt)
                }
                Err(e) => {
                    last_error = e;
                    1 << attempt
                }
            };
            if attempt + 1 < MAX_ATTEMPTS {
                std::thread::sleep(Duration::from_secs(wait.min(30)));
            }
        }
        Err(LlmError::new(last_error))
    }
}

/// Fournisseur de test : rejoue des réponses dans l'ordre et enregistre les requêtes.
#[derive(Default)]
pub struct Scripted {
    replies: RefCell<VecDeque<Response>>,
    requests: RefCell<Vec<Json>>,
}

impl Scripted {
    pub fn new(replies: impl IntoIterator<Item = Response>) -> Self {
        Scripted { replies: RefCell::new(replies.into_iter().collect()), requests: RefCell::default() }
    }

    /// Corps JSON de chaque requête reçue.
    pub fn requests(&self) -> Vec<Json> {
        self.requests.borrow().clone()
    }
}

impl Provider for Scripted {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        self.requests.borrow_mut().push(request_body(request));
        self.replies.borrow_mut().pop_front().ok_or_else(|| LlmError::new("plus de réponse scriptée"))
    }
}

impl<P: Provider + ?Sized> Provider for std::rc::Rc<P> {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        (**self).complete(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_has_tools_structured_output_and_effort() {
        let mut model = ModelConfig::new("anthropic", "claude-opus-5");
        model.effort = Some("low".into());
        assert!(model.fallbacks);
        assert!(!ModelConfig::new("anthropic", "claude-haiku-4-5").fallbacks);
        let request = Request {
            model: &model,
            system: Some("sys".into()),
            messages: vec![json!({"role": "user", "content": "hi"})],
            tools: vec![ToolSpec {
                name: "t".into(),
                description: "d".into(),
                input_schema: json!({"type": "object"}),
            }],
            output_schema: Some(json!({"type": "object"})),
        };
        let body = request_body(&request);
        assert_eq!(body["system"], "sys");
        assert_eq!(body["tools"][0]["strict"], true);
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["fallbacks"], "default");
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn costs() {
        let usage = Usage { input_tokens: 1_000_000, output_tokens: 1_000_000, ..Usage::default() };
        assert_eq!(cost_usd("claude-haiku-4-5-20251001", &usage), Some(6.0));
        assert_eq!(cost_usd("claude-opus-5-5", &usage), Some(24.0));
        assert_eq!(cost_usd("claude-opus-5", &usage), Some(30.0));
        assert_eq!(cost_usd("gpt-4", &usage), None);
    }

    #[test]
    fn parses_api_response() {
        let body = json!({
            "model": "claude-opus-5",
            "stop_reason": "tool_use",
            "content": [{"type": "text", "text": "Je lis."}, {"type": "tool_use", "id": "t1", "name": "read", "input": {"p": 1}}],
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 2}
        });
        let r = parse_response(&body).unwrap();
        assert_eq!(r.text(), "Je lis.");
        assert_eq!(r.tool_uses()[0].name, "read");
        assert_eq!(r.usage.total_tokens(), 17);
    }
}
