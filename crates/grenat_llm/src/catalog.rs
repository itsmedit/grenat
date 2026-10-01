//! The providers Grenat knows: how each is reached (its protocol, its URL),
//! where its key is found, whether it makes embeddings, takes audio in
//! prompts or transcribes it. A model names its provider (`provider:
//! openai`); nothing else is needed.

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
    /// The most tokens in one request, all texts together, as the provider
    /// documents it (a text longer goes alone); Grenat counts no tokens, so
    /// texts are measured by an estimate that holds for any script (see
    /// [`crate::embeddings_wire`]).
    pub max_tokens: usize,
}

/// How a provider takes audio in a prompt: an `input_audio` part of Chat
/// Completions (`{data, format}`, base64).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioInput {
    /// The formats it takes, as [`crate::audio`] names them (`wav`, `mp3`…).
    pub formats: &'static [&'static str],
    /// The most base64 text of audio a request may carry, where its
    /// request size is limited.
    pub max_encoded_bytes: Option<usize>,
}

/// How a provider transcribes: `POST <base_url>/audio/transcriptions`, the
/// file uploaded as `multipart/form-data` (see [`crate::transcription_wire`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TranscriptionApi {
    /// The largest file it takes, in bytes.
    pub max_bytes: usize,
    /// The formats it takes, as [`crate::audio`] names them.
    pub formats: &'static [&'static str],
}

const fn embeddings(dimensions_field: &'static str, max_inputs: usize, max_tokens: usize) -> Option<EmbeddingApi> {
    Some(EmbeddingApi { dimensions_field, max_inputs, max_tokens })
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
    /// A streamed answer asks for its usage (`stream_options: {"include_usage":
    /// true}`, Chat Completions); the others send it unasked, or would
    /// refuse the option.
    pub stream_usage: bool,
    /// Takes audio in prompts, if it does.
    pub audio_input: Option<AudioInput>,
    /// Its transcription API, if it has one.
    pub transcription: Option<TranscriptionApi>,
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
        stream_usage: true,
        audio_input: None,
        transcription: None,
    }
}

/// A megabyte, as providers count file sizes.
pub const MB: usize = 1024 * 1024;

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
        stream_usage: false,
        // its models take no audio: it is transcribed first
        audio_input: None,
        transcription: None,
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
        // 2,048 texts and 300,000 tokens a request, all models
        embeddings: embeddings("dimensions", 2048, 300_000),
        // the Responses API's last event carries the usage
        stream_usage: false,
        // through Chat Completions (the Responses API takes no audio), by
        // its audio models (`gpt-audio`…); its limit is not documented
        audio_input: Some(AudioInput { formats: &["wav", "mp3"], max_encoded_bytes: None }),
        // 25 MB (26,214,400 bytes); the formats its API reference lists
        // (no `.opus`: Opus audio goes up in an Ogg file, `.ogg`)
        transcription: Some(TranscriptionApi {
            max_bytes: 25 * MB,
            formats: &["flac", "mp3", "m4a", "ogg", "wav", "webm"],
        }),
    },
    // Google's OpenAI-compatible endpoint for Gemini
    Catalogued {
        max_tokens_field: "max_completion_tokens",
        // 100 texts a request; its token limit is not documented
        embeddings: embeddings("dimensions", 100, 100_000),
        // a request is 20 MB at most, prompt and files together
        audio_input: Some(AudioInput {
            formats: &["wav", "mp3", "aiff", "aac", "ogg", "flac"],
            max_encoded_bytes: Some(20_000_000),
        }),
        ..openai_like("gemini", "https://generativelanguage.googleapis.com/v1beta/openai", "GEMINI_API_KEY", true)
    },
    Catalogued {
        // its limits are not documented: small requests
        embeddings: embeddings("output_dimension", 64, 8_000),
        // its API documents no `stream_options`
        stream_usage: false,
        ..openai_like("mistral", "https://api.mistral.ai/v1", "MISTRAL_API_KEY", true)
    },
    openai_like("xai", "https://api.x.ai/v1", "XAI_API_KEY", true),
    openai_like("openrouter", "https://openrouter.ai/api/v1", "OPENROUTER_API_KEY", true),
    openai_like("groq", "https://api.groq.com/openai/v1", "GROQ_API_KEY", false),
    openai_like("deepseek", "https://api.deepseek.com", "DEEPSEEK_API_KEY", false),
    // its API documents no `stream_options`: the last chunk has the usage
    Catalogued {
        stream_usage: false,
        ..openai_like("together", "https://api.together.xyz/v1", "TOGETHER_API_KEY", false)
    },
    // local models; `base_url:` for another machine
    Catalogued {
        key_required: false,
        embeddings: embeddings("dimensions", 64, 100_000),
        ..openai_like("ollama", "http://localhost:11434/v1", "OLLAMA_API_KEY", false)
    },
    // embeddings only: Anthropic's recommended provider (1,000 texts a
    // request; 120,000 tokens for its large and code models, more for the
    // others: the least, whatever the model)
    Catalogued {
        protocol: Protocol::EmbeddingsOnly,
        embeddings: embeddings("output_dimension", 1000, 120_000),
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

/// The names of the providers that transcribe, for messages.
pub fn transcription_names() -> String {
    PROVIDERS.iter().filter(|p| p.transcription.is_some()).map(|p| p.name).collect::<Vec<_>>().join(", ")
}

/// The names of the providers that take audio in prompts, for messages.
pub fn audio_input_names() -> String {
    PROVIDERS.iter().filter(|p| p.audio_input.is_some()).map(|p| p.name).collect::<Vec<_>>().join(", ")
}

/// Why `provider` takes no audio in a prompt, if it takes none.
pub fn refuses_audio(provider: &Catalogued) -> Option<String> {
    if provider.audio_input.is_some() {
        return None;
    }
    let first = "transcribe it first (`transcribe(:whisper, audio)`) and give the model its text";
    Some(match provider.protocol {
        Protocol::Anthropic => format!("Anthropic's models take no audio: {first}"),
        _ => format!(
            "the provider `{}` takes no audio in prompts in Grenat (those that do: {}): {first}",
            provider.name,
            audio_input_names()
        ),
    })
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
        crate::ModelKind::Transcription if provider.protocol == Protocol::Anthropic => Some(format!(
            "Anthropic has no transcription API: declare an `openai` model (`gpt-transcribe`, `whisper-1`) (or one of {})",
            transcription_names()
        )),
        crate::ModelKind::Transcription if provider.transcription.is_none() => Some(format!(
            "the provider `{}` makes no transcriptions in Grenat (those that do: {})",
            provider.name,
            transcription_names()
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
