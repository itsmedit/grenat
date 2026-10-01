//! Requests, responses and model configuration; the [`Provider`] trait.

use serde_json::{Value as Json, json};

use crate::Price;

/// What a model is for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelKind {
    /// It answers prompts, agents and conversations.
    #[default]
    Chat,
    /// It turns texts into vectors (`embed`).
    Embedding,
}

/// Which requests carry prompt-cache breakpoints, where the provider wants
/// them written (Anthropic); the others cache by themselves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Caching {
    /// None (`cache: false`).
    Off,
    /// An agent's turns: its tools, its instructions and the history of its
    /// run, sent again at every turn (the default).
    #[default]
    Agents,
    /// Every request: also what repeats in prompts and conversations — the
    /// system prompt, the history before the last message, the documents
    /// and images given before a question (`cache: true`).
    Always,
}

/// How long a cache entry lives after its last use (Anthropic).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheTtl {
    /// Five minutes: writes cost 1.25 times the input price.
    #[default]
    FiveMinutes,
    /// An hour (`cache_ttl: "1h"`): writes cost twice the input price.
    OneHour,
}

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
    /// Another address for the provider (a proxy, a machine running Ollama).
    pub base_url: Option<String>,
    /// Dollars per million tokens, when Grenat does not know the model's
    /// price.
    pub price: Option<Price>,
    pub kind: ModelKind,
    /// The size of an embedding model's vectors, where the provider lets
    /// it be chosen (its default otherwise).
    pub dimensions: Option<u32>,
    /// Which requests mark what the prompt cache keeps (`cache:`).
    pub cache: Caching,
    /// How long it keeps it (`cache_ttl:`).
    pub cache_ttl: CacheTtl,
}

impl ModelConfig {
    pub fn new(provider: impl Into<String>, name: impl Into<String>) -> Self {
        let name = name.into();
        // recommended for models whose classifiers may refuse a request
        let fallbacks = matches!(name.as_str(), "claude-opus-5" | "claude-fable-5-1");
        ModelConfig {
            provider: provider.into(),
            name,
            max_tokens: 16_000,
            temperature: None,
            effort: None,
            fallbacks,
            base_url: None,
            price: None,
            kind: ModelKind::Chat,
            dimensions: None,
            cache: Caching::default(),
            cache_ttl: CacheTtl::default(),
        }
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

/// The tokens of a call. The input is split three ways: `input_tokens` at
/// the full price, `cache_creation_input_tokens` written to the prompt
/// cache, `cache_read_input_tokens` read from it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    /// Of the tokens written, those kept for an hour (priced higher).
    pub cache_creation_1h_input_tokens: u64,
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.prompt_tokens() + self.output_tokens
    }

    /// From an input counted whole, of which `read` tokens came from the
    /// cache and `written` went to it (OpenAI's way of saying it).
    pub fn from_whole_input(input: u64, read: u64, written: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input.saturating_sub(read).saturating_sub(written),
            output_tokens: output,
            cache_creation_input_tokens: written,
            cache_read_input_tokens: read,
            cache_creation_1h_input_tokens: 0,
        }
    }

    /// Every input token, cached or not.
    pub fn prompt_tokens(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
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

/// Texts to turn into vectors, in one request.
#[derive(Debug, Clone)]
pub struct EmbeddingRequest<'a> {
    pub model: &'a ModelConfig,
    pub inputs: Vec<String>,
}

/// The vectors of an [`EmbeddingRequest`], in the order of its inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct Embeddings {
    pub vectors: Vec<Vec<f64>>,
    /// Input tokens only: an embedding has no output.
    pub usage: Usage,
    pub model: String,
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

    /// The answer as the model writes it: each piece given to `sink` as it
    /// arrives, then the whole response (usage, tool calls), as
    /// [`Provider::complete`] returns it. A provider that does not stream
    /// gives its answer as one piece.
    fn stream(&self, request: &Request, sink: &mut crate::Sink) -> Result<Response, LlmError> {
        let response = self.complete(request)?;
        crate::streaming::replay(&response, sink)?;
        Ok(response)
    }

    /// Several requests at once, answered in the same order: through a
    /// batch API where the provider has one (cheaper, slower), one by one
    /// otherwise.
    fn batch(&self, requests: &[Request]) -> Result<Vec<Result<Response, LlmError>>, LlmError> {
        Ok(requests.iter().map(|r| self.complete(r)).collect())
    }

    /// The vectors of texts, from an embedding model.
    fn embed(&self, request: &EmbeddingRequest) -> Result<Embeddings, LlmError> {
        Err(LlmError::new(format!("`{}` makes no embeddings here", request.model.name)))
    }
}

impl<P: Provider + ?Sized> Provider for std::sync::Arc<P> {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        (**self).complete(request)
    }

    fn stream(&self, request: &Request, sink: &mut crate::Sink) -> Result<Response, LlmError> {
        (**self).stream(request, sink)
    }

    fn batch(&self, requests: &[Request]) -> Result<Vec<Result<Response, LlmError>>, LlmError> {
        (**self).batch(requests)
    }

    fn embed(&self, request: &EmbeddingRequest) -> Result<Embeddings, LlmError> {
        (**self).embed(request)
    }
}
