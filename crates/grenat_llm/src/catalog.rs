//! The providers Grenat knows: how each is reached (its protocol, its URL),
//! where its key is found and whether it makes embeddings. A model names
//! its provider (`provider: openai`); nothing else is needed.

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
    /// No chat: a provider of embeddings only (Voyage).
    EmbeddingsOnly,
}

/// How a provider makes embeddings: `POST <base_url>/embeddings`, in
/// OpenAI's format (`{model, input}` in, `{data: [{embedding, index}],
/// usage}` out), with these differences.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmbeddingApi {
    /// The field choosing the vectors' size (`dimensions`, `output_dimension`).
    pub dimensions_field: &'static str,
    /// The most texts in one request.
    pub max_inputs: usize,
    /// The most characters in one request (a text longer goes alone): its
    /// limit in tokens, about four characters each, with a margin.
    pub max_chars: usize,
}

const fn embeddings(dimensions_field: &'static str, max_inputs: usize, max_chars: usize) -> Option<EmbeddingApi> {
    Some(EmbeddingApi { dimensions_field, max_inputs, max_chars })
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
    /// Its embeddings API, if it has one.
    pub embeddings: Option<EmbeddingApi>,
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
        embeddings: None,
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
        // none: Anthropic recommends Voyage AI
        embeddings: None,
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
        // 2,048 texts and 300,000 tokens a request
        embeddings: embeddings("dimensions", 2048, 800_000),
    },
    // Google's OpenAI-compatible endpoint for Gemini
    Catalogued {
        max_tokens_field: "max_completion_tokens",
        embeddings: embeddings("dimensions", 100, 400_000),
        ..openai_like("gemini", "https://generativelanguage.googleapis.com/v1beta/openai", "GEMINI_API_KEY", true)
    },
    Catalogued {
        // its limits are not documented: small requests
        embeddings: embeddings("output_dimension", 64, 32_000),
        ..openai_like("mistral", "https://api.mistral.ai/v1", "MISTRAL_API_KEY", true)
    },
    openai_like("xai", "https://api.x.ai/v1", "XAI_API_KEY", true),
    openai_like("openrouter", "https://openrouter.ai/api/v1", "OPENROUTER_API_KEY", true),
    openai_like("groq", "https://api.groq.com/openai/v1", "GROQ_API_KEY", false),
    openai_like("deepseek", "https://api.deepseek.com", "DEEPSEEK_API_KEY", false),
    openai_like("together", "https://api.together.xyz/v1", "TOGETHER_API_KEY", false),
    // local models; `base_url:` for another machine
    Catalogued {
        key_required: false,
        embeddings: embeddings("dimensions", 64, 400_000),
        ..openai_like("ollama", "http://localhost:11434/v1", "OLLAMA_API_KEY", false)
    },
    // embeddings only: Anthropic's recommended provider (1,000 texts and
    // at least 120,000 tokens a request)
    Catalogued {
        protocol: Protocol::EmbeddingsOnly,
        embeddings: embeddings("output_dimension", 1000, 400_000),
        ..openai_like("voyage", "https://api.voyageai.com/v1", "VOYAGE_API_KEY", false)
    },
];

pub fn provider(name: &str) -> Option<&'static Catalogued> {
    PROVIDERS.iter().find(|p| p.name == name)
}

/// The providers' names, for messages.
pub fn names() -> String {
    PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")
}

/// The names of the providers that make embeddings, for messages.
pub fn embedding_names() -> String {
    PROVIDERS.iter().filter(|p| p.embeddings.is_some()).map(|p| p.name).collect::<Vec<_>>().join(", ")
}

/// Why `provider` cannot serve a model of `kind`, if it cannot.
pub fn refuses(provider: &Catalogued, kind: crate::ModelKind) -> Option<String> {
    match kind {
        crate::ModelKind::Embedding if provider.protocol == Protocol::Anthropic => Some(format!(
            "Anthropic has no embeddings API: declare a `voyage` model, the provider it recommends (or one of {})",
            embedding_names()
        )),
        crate::ModelKind::Embedding if provider.embeddings.is_none() => Some(format!(
            "the provider `{}` makes no embeddings in Grenat (those that do: {})",
            provider.name,
            embedding_names()
        )),
        crate::ModelKind::Chat if provider.protocol == Protocol::EmbeddingsOnly => {
            Some(format!("the provider `{}` only makes embeddings: add `kind: :embedding`", provider.name))
        }
        _ => None,
    }
}

/// The size of a model's vectors when none is asked, for the models whose
/// providers document it (the fake embeddings of tests take it).
pub fn default_dimensions(model: &str) -> Option<u32> {
    const SIZES: &[(&str, u32)] = &[
        ("text-embedding-3-small", 1536),
        ("text-embedding-3-large", 3072),
        ("text-embedding-ada-002", 1536),
        ("gemini-embedding", 3072),
        ("mistral-embed", 1024),
        ("voyage-", 1024),
        ("nomic-embed-text", 768),
        ("mxbai-embed-large", 1024),
    ];
    SIZES.iter().find(|(prefix, _)| model.starts_with(prefix)).map(|&(_, size)| size)
}
