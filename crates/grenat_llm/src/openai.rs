//! HTTP client for OpenAI (its Responses API) and for the providers that
//! speak its Chat Completions: Gemini (Google's compatible endpoint),
//! Mistral, xAI, OpenRouter, Groq, DeepSeek, Together, Ollama. Embeddings
//! go through its `/embeddings`, which Voyage speaks too; transcriptions
//! through its `/audio/transcriptions`. A request carrying audio goes to
//! OpenAI's Chat Completions: its Responses API takes none. Streamed answers
//! are read by [`crate::responses_stream`] and [`crate::chat_stream`].

use std::time::Duration;

use serde_json::{Value as Json, json};

use crate::catalog::{Catalogued, Protocol};
use crate::chat_stream::ChatDecoder;
use crate::embeddings_wire::{chunks, embeddings_body, parse_embeddings};
use crate::multipart::Form;
use crate::openai_wire::{chat_body, parse_chat};
use crate::responses_stream::ResponsesDecoder;
use crate::responses_wire::{parse_responses, responses_body};
use crate::streaming::{Decoder, Failed};
use crate::transcription_wire::{filename, form_fields, parse_transcript};
use crate::*;

pub struct OpenAi {
    agent: ureq::Agent,
    provider: Catalogued,
    /// `None` for a local server that needs none.
    api_key: Option<String>,
    base_url: String,
    retry_delay: Duration,
}

impl OpenAi {
    /// A client of `provider` at `base_url` (its own by default).
    pub fn new(provider: Catalogued, api_key: Option<String>, base_url: Option<&str>) -> OpenAi {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(600)))
            .build()
            .into();
        let base_url = base_url.unwrap_or(provider.base_url).trim_end_matches('/').to_string();
        OpenAi { agent, provider, api_key, base_url, retry_delay: Duration::from_secs(1) }
    }

    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// `POST <path>` with a JSON body: (status, `retry-after` seconds, the body unread).
    fn post(&self, path: &str, body: &Json) -> Result<(u16, Option<u64>, ureq::Body), String> {
        self.post_bytes(path, "application/json", &body.to_string().into_bytes())
    }

    /// `POST <path>` with a body of `content_type`.
    fn post_bytes(
        &self,
        path: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<(u16, Option<u64>, ureq::Body), String> {
        let mut request = self.agent.post(format!("{}{path}", self.base_url)).header("content-type", content_type);
        if let Some(key) = &self.api_key {
            request = request.header("authorization", &format!("Bearer {key}"));
        }
        let response = request.send(body).map_err(|e| format!("connection failed: {e}"))?;
        let status = response.status().as_u16();
        let retry_after =
            response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok());
        Ok((status, retry_after, response.into_body()))
    }

    fn send(&self, path: &str, body: &Json) -> crate::retry::Attempt {
        read_json(self.post(path, body)?)
    }

    /// The provider as Chat Completions speaks to it: OpenAI's own for a
    /// request carrying audio, which its Responses API does not take.
    fn chat_provider(&self, request: &Request) -> Option<Catalogued> {
        match self.provider.protocol {
            Protocol::Responses if crate::audio::in_messages(&request.messages) => Some(Catalogued {
                protocol: Protocol::OpenAi,
                max_tokens_field: "max_completion_tokens",
                stream_usage: true,
                ..self.provider
            }),
            Protocol::Responses => None,
            _ => Some(self.provider),
        }
    }

    /// One streamed attempt.
    fn stream_once(
        &self,
        path: &str,
        body: &Json,
        decoder: &mut impl Decoder,
        sink: &mut Sink,
    ) -> Result<Response, Failed> {
        let (status, retry_after, mut response) = self.post(path, body).map_err(Failed::retry)?;
        if status != 200 {
            let text = response.read_to_string().unwrap_or_default();
            return Err(Failed::status(status, retry_after, &text));
        }
        crate::streaming::decode(response.into_reader(), decoder, sink)
    }
}

impl Provider for OpenAi {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        if let Some(refusal) = crate::catalog::refuses(&self.provider, request.model.kind) {
            return Err(LlmError::new(refusal));
        }
        let Some(chat) = self.chat_provider(request) else {
            let body = responses_body(request, &self.provider)?;
            return crate::retry::with_retries(self.retry_delay, || self.send("/responses", &body), parse_responses);
        };
        let body = chat_body(request, &chat)?;
        crate::retry::with_retries(self.retry_delay, || self.send("/chat/completions", &body), parse_chat)
    }

    /// Each attempt reads the stream with a decoder of its own.
    fn stream(&self, request: &Request, sink: &mut Sink) -> Result<Response, LlmError> {
        if let Some(refusal) = crate::catalog::refuses(&self.provider, request.model.kind) {
            return Err(LlmError::new(refusal));
        }
        let Some(chat) = self.chat_provider(request) else {
            let mut body = responses_body(request, &self.provider)?;
            body["stream"] = json!(true);
            return crate::retry::with_stream_retries(self.retry_delay, || {
                self.stream_once("/responses", &body, &mut ResponsesDecoder::default(), sink)
            });
        };
        let mut body = chat_body(request, &chat)?;
        body["stream"] = json!(true);
        if chat.stream_usage {
            body["stream_options"] = json!({"include_usage": true});
        }
        crate::retry::with_stream_retries(self.retry_delay, || {
            self.stream_once("/chat/completions", &body, &mut ChatDecoder::default(), sink)
        })
    }

    /// Several requests when the texts are many: their vectors and tokens put together.
    fn embed(&self, request: &EmbeddingRequest) -> Result<Embeddings, LlmError> {
        let Some(api) = self.provider.embeddings else {
            return Err(LlmError::new(
                crate::catalog::refuses(&self.provider, ModelKind::Embedding).unwrap_or_default(),
            ));
        };
        let mut all = Embeddings { vectors: Vec::new(), usage: Usage::default(), model: request.model.name.clone() };
        for range in chunks(&request.inputs, &api) {
            let part = EmbeddingRequest { model: request.model, inputs: request.inputs[range].to_vec() };
            let body = embeddings_body(&part, &api);
            let expected = part.inputs.len();
            let answer = crate::retry::with_retries(
                self.retry_delay,
                || self.send("/embeddings", &body),
                |json| parse_embeddings(json, expected),
            )?;
            all.vectors.extend(answer.vectors);
            all.usage.input_tokens += answer.usage.input_tokens;
            if !answer.model.is_empty() {
                all.model = answer.model;
            }
        }
        Ok(all)
    }

    /// The audio uploaded as a form, once checked: a file the provider would
    /// refuse is never sent.
    fn transcribe(&self, request: &TranscriptionRequest) -> Result<Transcript, LlmError> {
        let Some(api) = self.provider.transcription else {
            return Err(LlmError::new(
                crate::catalog::refuses(&self.provider, ModelKind::Transcription).unwrap_or_default(),
            ));
        };
        crate::transcription_wire::check(request, &api)?;
        let mut form = Form::default();
        for (name, value) in form_fields(request) {
            form.text(name, &value);
        }
        let audio = crate::audio::decode_base64(&request.data)?;
        form.file("file", &filename(&request.media_type), &request.media_type, audio);
        let (content_type, body) = form.finish();
        crate::retry::with_retries(
            self.retry_delay,
            || read_json(self.post_bytes("/audio/transcriptions", &content_type, &body)?),
            |json| parse_transcript(json, request),
        )
    }
}

/// An answer read whole: its JSON, or its text when it is not JSON.
fn read_json((status, retry_after, mut response): (u16, Option<u64>, ureq::Body)) -> crate::retry::Attempt {
    let text = response.read_to_string().map_err(|e| format!("unreadable response: {e}"))?;
    let json = serde_json::from_str(&text).unwrap_or(Json::String(text));
    Ok((status, retry_after, json))
}
