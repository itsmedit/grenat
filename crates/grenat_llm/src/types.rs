//! Requests, responses and model configuration; the [`Provider`] trait.

use serde_json::{Value as Json, json};

/// Model configuration, from a `model :name, …` declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelConfig {
    pub provider: String,
    pub name: String,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
    /// `low` … `max` (`output_config.effort`).
    pub effort: Option<String>,
    /// Server-side fallback when the model refuses (`fallbacks: "default"`).
    pub fallbacks: bool,
}

impl ModelConfig {
    pub fn new(provider: impl Into<String>, name: impl Into<String>) -> Self {
        let name = name.into();
        // recommended for models whose classifiers may refuse a request
        let fallbacks = matches!(name.as_str(), "claude-opus-5" | "claude-fable-5-1");
        ModelConfig { provider: provider.into(), name, max_tokens: 16_000, temperature: None, effort: None, fallbacks }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Json,
    /// The model must follow the schema exactly (the schemas Grenat
    /// generates are written for it; an MCP server's may not be).
    pub strict: bool,
}

#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub model: &'a ModelConfig,
    pub system: Option<String>,
    /// Messages in API format (`{"role": …, "content": …}`).
    pub messages: Vec<Json>,
    pub tools: Vec<ToolSpec>,
    /// Structured output: JSON Schema of the expected reply.
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
    /// Raw content blocks, to be sent back unchanged in the history.
    pub content: Vec<Json>,
    pub stop_reason: String,
    pub usage: Usage,
    pub model: String,
}

impl Response {
    /// Concatenation of the `text` blocks.
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

    // ── Constructors for tests ──

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
    pub(crate) fn new(message: impl Into<String>) -> Self {
        LlmError { message: message.into() }
    }
}

/// Shared between the interpreter's concurrent tasks.
pub trait Provider: Send + Sync {
    fn complete(&self, request: &Request) -> Result<Response, LlmError>;
}

impl<P: Provider + ?Sized> Provider for std::sync::Arc<P> {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        (**self).complete(request)
    }
}
