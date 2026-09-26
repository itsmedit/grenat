//! The providers Grenat knows: how each is reached (its protocol, its URL)
//! and where its key is found. A model names its provider
//! (`provider: openai`); nothing else is needed.

/// How a provider is spoken to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Anthropic's Messages API.
    Anthropic,
    /// OpenAI's Chat Completions, which most providers also speak.
    OpenAi,
    /// OpenAI's Responses API: its own models (reasoning models take tools
    /// there, not through Chat Completions).
    Responses,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Catalogued {
    pub name: &'static str,
    pub protocol: Protocol,
    pub base_url: &'static str,
    /// The environment variable holding the key (credentials first:
    /// `<name>.api_key`).
    pub key_variable: &'static str,
    /// Local servers need none.
    pub key_required: bool,
    /// Follows a JSON Schema exactly (`response_format: json_schema`, strict
    /// tools); otherwise JSON mode, the schema given in the instructions.
    pub strict_schemas: bool,
    /// Reads PDF documents.
    pub documents: bool,
    /// `max_completion_tokens` (OpenAI's current field) or `max_tokens`.
    pub max_tokens_field: &'static str,
}

const fn openai_like(
    name: &'static str,
    base_url: &'static str,
    key_variable: &'static str,
    strict_schemas: bool,
) -> Catalogued {
    Catalogued {
        name,
        protocol: Protocol::OpenAi,
        base_url,
        key_variable,
        key_required: true,
        strict_schemas,
        documents: false,
        max_tokens_field: "max_tokens",
    }
}

pub const PROVIDERS: &[Catalogued] = &[
    Catalogued {
        name: "anthropic",
        protocol: Protocol::Anthropic,
        base_url: "https://api.anthropic.com",
        key_variable: "ANTHROPIC_API_KEY",
        key_required: true,
        strict_schemas: true,
        documents: true,
        max_tokens_field: "max_tokens",
    },
    Catalogued {
        name: "openai",
        protocol: Protocol::Responses,
        base_url: "https://api.openai.com/v1",
        key_variable: "OPENAI_API_KEY",
        key_required: true,
        strict_schemas: true,
        documents: true,
        max_tokens_field: "max_output_tokens",
    },
    // Google's OpenAI-compatible endpoint for Gemini
    Catalogued {
        max_tokens_field: "max_completion_tokens",
        ..openai_like("gemini", "https://generativelanguage.googleapis.com/v1beta/openai", "GEMINI_API_KEY", true)
    },
    openai_like("mistral", "https://api.mistral.ai/v1", "MISTRAL_API_KEY", true),
    openai_like("xai", "https://api.x.ai/v1", "XAI_API_KEY", true),
    openai_like("openrouter", "https://openrouter.ai/api/v1", "OPENROUTER_API_KEY", true),
    openai_like("groq", "https://api.groq.com/openai/v1", "GROQ_API_KEY", false),
    openai_like("deepseek", "https://api.deepseek.com", "DEEPSEEK_API_KEY", false),
    openai_like("together", "https://api.together.xyz/v1", "TOGETHER_API_KEY", false),
    // local models; `base_url:` for another machine
    Catalogued { key_required: false, ..openai_like("ollama", "http://localhost:11434/v1", "OLLAMA_API_KEY", false) },
];

pub fn provider(name: &str) -> Option<&'static Catalogued> {
    PROVIDERS.iter().find(|p| p.name == name)
}

/// The providers' names, for messages.
pub fn names() -> String {
    PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")
}
