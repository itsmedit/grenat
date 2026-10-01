//! Transcriptions: `transcribe(:whisper, audio)` is the text of a recording,
//! `~String` — what was said comes from outside, untrusted as a model's
//! answer is; `transcribe(:whisper, audio, segments: true)` its timed
//! segments, `Array(TranscriptSegment)` (`start`, `end` in seconds, `text`
//! and `speaker` untrusted). Options: `language: "fr"`, `prompt: "…"`
//! (names, terms), `keywords: [...]` (`gpt-transcribe`).
//!
//! The model is declared with `kind: :transcription`; the call is an `llm`
//! effect, counted by budgets and recorded in the ledger with its cost — by
//! the minute or by tokens, as the model is billed (`price: {minute: …}`
//! for one Grenat does not know). In tests, `mock_transcribe :whisper,
//! text: "…"` (or `replies: [...]`, a text or segments each) stands for it.

use grenat_llm::{
    FakeTranscripts, ModelConfig, ModelKind, Provider, Segment, Transcript, TranscriptReply, TranscriptionRequest,
    cost_at,
};

use crate::builtins::{ATTACHMENT, number};
use crate::prelude::*;
use crate::value::money;

/// The record a segment is.
pub(crate) const TRANSCRIPT_SEGMENT: &str = "TranscriptSegment";

/// The options of `transcribe`, read from its named arguments.
#[derive(Default)]
struct Options {
    language: Option<String>,
    prompt: Option<String>,
    keywords: Vec<String>,
    segments: bool,
}

impl<'p> Interp<'p> {
    /// `transcribe(:whisper, audio, …)`; without a model, the first
    /// transcription model declared.
    pub(crate) fn transcribe(&mut self, model: Option<&str>, audio: &Value<'p>, args: &Args<'p>) -> R<'p> {
        self.check_effect("llm")?;
        let named: Vec<Value<'p>> = args.named.iter().map(|(_, v)| v.clone()).collect();
        crate::eval::secrets::not_for_models(&named)?;
        let model = self.transcription_model(model)?;
        let (media_type, data) = audio_of(audio)?;
        let options = options(args)?;
        let request = TranscriptionRequest {
            model: &model,
            media_type,
            data,
            language: options.language,
            prompt: options.prompt,
            keywords: options.keywords,
            segments: options.segments,
        };
        let transcript = self.transcription_call(&request)?;
        if !request.segments {
            return Ok(Value::str(transcript.text).taint());
        }
        Ok(Value::array(transcript.segments.into_iter().map(segment).collect()))
    }

    /// The model `name` names, which must transcribe; else the first that does.
    fn transcription_model(&self, name: Option<&str>) -> Result<ModelConfig, Ctrl<'p>> {
        let Some(name) = name else {
            let first =
                self.models.borrow().iter().find(|(_, c)| c.kind == ModelKind::Transcription).map(|(_, c)| c.clone());
            return first.map_or_else(
                || {
                    raise(
                        "LlmError",
                        "no transcription model declared: add `model :whisper, provider: :openai, name: \"whisper-1\", kind: :transcription`",
                    )
                },
                Ok,
            );
        };
        let model = self.model_named(name)?;
        if model.kind != ModelKind::Transcription {
            return raise(
                "LlmError",
                format!("`:{name}` is not a transcription model: declare one with `kind: :transcription`"),
            );
        }
        Ok(model)
    }

    /// The transcript of `request`, accounted: budgets, the ledger, the log.
    fn transcription_call(&mut self, request: &TranscriptionRequest) -> Result<Transcript, Ctrl<'p>> {
        self.check_cancel()?;
        self.check_budgets()?;
        let started = Instant::now();
        let (provider, fake) = self.transcriber(request.model)?;
        let transcript = match grenat_green::blocking(|| provider.transcribe(request)) {
            Ok(transcript) => transcript,
            Err(e) => return raise("LlmError", e.message),
        };
        self.llm_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cost = if fake { 0.0 } else { self.transcription_cost(request.model, &transcript) };
        for budget in &self.budgets {
            budget.add(cost, transcript.usage.total_tokens());
        }
        self.record_call(&request.model.name, &transcript.usage, cost);
        if self.log {
            let measure = match transcript.seconds {
                Some(seconds) => format!("{:.1} min", seconds / 60.0),
                None => format!("{} in / {} out", transcript.usage.input_tokens, transcript.usage.output_tokens),
            };
            let line = format!(
                "[transcribe] {} · {measure} · {} · {:.1}s\n",
                request.model.name,
                money(cost),
                started.elapsed().as_secs_f64()
            );
            self.write_err(&line);
        }
        self.check_budgets()?;
        Ok(transcript)
    }

    /// By the minute at `price: {minute: …}`, by tokens at `price: {input:
    /// …, output: …}`, else at the price Grenat knows; an unknown one counts
    /// nothing, said once.
    fn transcription_cost(&mut self, model: &ModelConfig, transcript: &Transcript) -> f64 {
        if let (Some(per_minute), Some(seconds)) = (model.minute_price, transcript.seconds) {
            return seconds / 60.0 * per_minute;
        }
        if let Some(price) = model.price
            && transcript.usage.total_tokens() > 0
        {
            return cost_at(price, &transcript.usage);
        }
        if let Some(cost) = grenat_llm::transcription_cost(&model.name, transcript) {
            return cost;
        }
        if self.unpriced.borrow_mut().insert(model.name.clone()) {
            self.write_err(&format!(
                "warning: the cost of `{}`'s transcriptions is unknown (its price, or the audio's duration): budgets in dollars do not count it (give `price: {{minute: …}}` or `price: {{input: …, output: …}}` in config/models.yml)\n",
                model.name
            ));
        }
        0.0
    }

    /// Who transcribes, and whether it is a fake: the model's fake, else
    /// the forced provider, else the real one (never in offline runs).
    fn transcriber(&mut self, model: &ModelConfig) -> Result<(Arc<dyn Provider>, bool), Ctrl<'p>> {
        let fake = {
            let fakes = self.transcription_mocks.borrow();
            let own = fakes.iter().find(|(m, _)| m.as_ref() == Some(model));
            own.or_else(|| fakes.iter().find(|(m, _)| m.is_none())).map(|(_, f)| f.clone())
        };
        if let Some(fake) = fake {
            return Ok((fake, true));
        }
        if self.provider.borrow().is_none() && self.offline {
            return raise(
                "LlmError",
                format!("no real model in tests: `{}` transcribes outside `mock_transcribe`", model.name),
            );
        }
        Ok((self.real_provider(model)?, false))
    }

    /// `mock_transcribe :whisper, text: "…"` (every call) or `replies: [...]`
    /// (in order: a text, segments — `[{start:, end:, text:, speaker:}]` —
    /// or an error), for that model or, without a name, every one.
    pub(crate) fn mock_transcribe(&mut self, model: Option<&str>, args: &Args<'p>) -> R<'p> {
        let config = match model {
            Some(name) => Some(self.transcription_model(Some(name))?),
            None => None,
        };
        let mut replies = Vec::new();
        let mut always = None;
        for (option, value) in &args.named {
            match (option.as_str(), value.untainted()) {
                ("text", Value::Str(text)) => always = Some(TranscriptReply::Text(text.to_string())),
                ("replies", Value::Array(items)) => {
                    for item in items.borrow().iter() {
                        replies.push(reply(item)?);
                    }
                }
                (option, v) => {
                    return raise(
                        "ArgumentError",
                        format!("invalid `mock_transcribe` option `{option}: {}`", v.inspect()),
                    );
                }
            }
        }
        let label = model.map_or_else(|| "every transcription model".to_string(), |name| format!("`:{name}`"));
        let fake = Arc::new(FakeTranscripts::new(label, replies, always));
        // the latest fake of a model replaces the previous one
        let mut fakes = self.transcription_mocks.borrow_mut();
        fakes.retain(|(m, _)| *m != config);
        fakes.push((config, fake));
        Ok(Value::Nil)
    }
}

/// The media type and base64 data of an audio attachment.
fn audio_of<'p>(value: &Value<'p>) -> Result<(String, String), Ctrl<'p>> {
    let expected = || {
        raise(
            "TypeError",
            format!("`transcribe` expects audio (`Audio.read(path)`, `Audio.url(url)`), got {}", value.inspect()),
        )
    };
    let Value::Record(record) = value.untainted() else { return expected() };
    let get = |name: &str| record.fields.iter().find(|(k, _)| &**k == name).map(|(_, v)| v.to_display());
    if &*record.ty != ATTACHMENT
        || get("kind").as_deref() != Some("audio")
        || get("source").as_deref() != Some("base64")
    {
        return expected();
    }
    crate::builtins::trusted_attachment(record)?;
    Ok((get("media_type").unwrap_or_default(), get("data").unwrap_or_default()))
}

fn options<'p>(args: &Args<'p>) -> Result<Options, Ctrl<'p>> {
    let mut options = Options::default();
    for (option, value) in &args.named {
        match (option.as_str(), value.untainted()) {
            ("language", Value::Str(s) | Value::Symbol(s)) => options.language = Some(s.to_string()),
            ("prompt", Value::Str(s)) => options.prompt = Some(s.to_string()),
            ("segments", Value::Bool(b)) => options.segments = *b,
            ("keywords", Value::Array(items)) => {
                options.keywords = items.borrow().iter().map(Value::to_display).collect();
            }
            (option, v) => {
                return raise("ArgumentError", format!("invalid `transcribe` option `{option}: {}`", v.inspect()));
            }
        }
    }
    Ok(options)
}

/// A segment as a record; what was said, and by whom, untrusted.
fn segment<'p>(segment: Segment) -> Value<'p> {
    Value::record(
        TRANSCRIPT_SEGMENT,
        vec![
            ("start".into(), Value::Float(segment.start)),
            ("end".into(), Value::Float(segment.end)),
            ("text".into(), Value::str(segment.text).taint()),
            ("speaker".into(), segment.speaker.map_or(Value::Nil, |s| Value::str(s).taint())),
        ],
    )
}

/// A reply of `mock_transcribe`: a text, segments, or an error.
fn reply<'p>(value: &Value<'p>) -> Result<TranscriptReply, Ctrl<'p>> {
    Ok(match value.untainted() {
        Value::Str(text) => TranscriptReply::Text(text.to_string()),
        Value::Error(e) => TranscriptReply::Error(e.message.clone()),
        Value::Array(items) => {
            let mut segments = Vec::new();
            for item in items.borrow().iter() {
                segments.push(given_segment(item)?);
            }
            TranscriptReply::Segments(segments)
        }
        other => {
            return raise(
                "TypeError",
                format!(
                    "a `mock_transcribe` reply is a text or segments (`[{{start:, end:, text:}}]`), got {}",
                    other.inspect()
                ),
            );
        }
    })
}

/// `{start: 0.0, end: 4.2, text: "…", speaker: "A"}`.
fn given_segment<'p>(value: &Value<'p>) -> Result<Segment, Ctrl<'p>> {
    let invalid =
        || raise("TypeError", format!("a segment is `{{start:, end:, text:, speaker:}}`, got {}", value.inspect()));
    let Value::Hash(pairs) = value.untainted() else { return invalid() };
    let pairs = pairs.borrow();
    let get = |name: &str| pairs.iter().find(|(k, _)| k.to_display() == name).map(|(_, v)| v.clone());
    let (Some(text), start, end) =
        (get("text"), get("start").as_ref().and_then(number), get("end").as_ref().and_then(number))
    else {
        return invalid();
    };
    Ok(Segment {
        start: start.unwrap_or(0.0),
        end: end.unwrap_or(0.0),
        text: text.to_display(),
        speaker: get("speaker").filter(|s| !matches!(s, Value::Nil)).map(|s| s.to_display()),
    })
}
