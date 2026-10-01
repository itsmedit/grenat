//! Transcription wire format — OpenAI's `POST /audio/transcriptions`: the
//! form sent with the file (the fields each model takes), what is checked
//! before anything is uploaded (the file's size and format, the options the
//! model has), and the answer read back as a [`Transcript`].
//!
//! The models differ: `whisper-1` gives timed segments (`verbose_json`,
//! `timestamp_granularities[]=segment`); `gpt-4o-transcribe-diarize` gives
//! them with speakers (`diarized_json`, `chunking_strategy=auto`) and takes
//! no prompt; `gpt-transcribe` takes `languages[]` and `keywords[]`; the
//! others give the text only.

use serde_json::Value as Json;

use crate::catalog::TranscriptionApi;
use crate::*;

/// What a model can be asked, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `whisper-1`: segments, `language`, `prompt`.
    Whisper,
    /// `gpt-4o-transcribe-diarize`: segments with speakers, `language`.
    Diarize,
    /// `gpt-transcribe`: `languages[]`, `prompt`, `keywords[]`.
    Transcribe,
    /// `gpt-4o-transcribe`, `gpt-4o-mini-transcribe`…: `language`, `prompt`.
    TextOnly,
}

fn family(model: &str) -> Family {
    if model.starts_with("whisper") {
        Family::Whisper
    } else if model.contains("diarize") {
        Family::Diarize
    } else if model.starts_with("gpt-transcribe") {
        Family::Transcribe
    } else {
        Family::TextOnly
    }
}

/// What the provider would refuse, said before uploading: a format it does
/// not read, a file too large, an option the model does not take.
pub(crate) fn check(request: &TranscriptionRequest, api: &TranscriptionApi) -> Result<(), LlmError> {
    let model = &request.model.name;
    if !crate::audio::format_of(&request.media_type).is_some_and(|f| api.formats.contains(&f)) {
        return Err(LlmError::new(format!(
            "`{model}` transcribes {} audio, not `{}`",
            api.formats.join(", "),
            request.media_type
        )));
    }
    let size = crate::audio::decoded_len(&request.data);
    if size > api.max_bytes {
        return Err(LlmError::new(format!(
            "the audio is {size} bytes ({}): `{model}` takes {} bytes at most — compress it (mp3, m4a) or split it, and transcribe each part",
            crate::audio::megabytes(size),
            api.max_bytes
        )));
    }
    let family = family(model);
    if request.segments && !matches!(family, Family::Whisper | Family::Diarize) {
        return Err(LlmError::new(format!(
            "`{model}` gives no timestamps: `segments: true` takes `whisper-1` (segments) or `gpt-4o-transcribe-diarize` (segments and speakers)"
        )));
    }
    if request.prompt.is_some() && family == Family::Diarize {
        return Err(LlmError::new(format!("`{model}` takes no `prompt:`")));
    }
    if !request.keywords.is_empty() && family != Family::Transcribe {
        return Err(LlmError::new(format!("`{model}` takes no `keywords:` (`gpt-transcribe` does)")));
    }
    Ok(())
}

/// The form's text fields, in order (the file goes last).
pub(crate) fn form_fields(request: &TranscriptionRequest) -> Vec<(&'static str, String)> {
    let family = family(&request.model.name);
    let mut fields = vec![("model", request.model.name.clone())];
    let format = match (family, request.segments) {
        (Family::Whisper, true) => "verbose_json",
        (Family::Diarize, true) => "diarized_json",
        _ => "json",
    };
    fields.push(("response_format", format.to_string()));
    if family == Family::Whisper && request.segments {
        fields.push(("timestamp_granularities[]", "segment".into()));
    }
    if family == Family::Diarize {
        // required for audio longer than 30 seconds
        fields.push(("chunking_strategy", "auto".into()));
    }
    if let Some(language) = &request.language {
        // `gpt-transcribe` takes several, in the field replacing `language`
        let field = if family == Family::Transcribe { "languages[]" } else { "language" };
        fields.push((field, language.clone()));
    }
    if let Some(prompt) = &request.prompt {
        fields.push(("prompt", prompt.clone()));
    }
    for keyword in &request.keywords {
        fields.push(("keywords[]", keyword.clone()));
    }
    if let Some(temperature) = request.model.temperature {
        fields.push(("temperature", temperature.to_string()));
    }
    fields
}

/// The name the file is uploaded under: the provider reads its format from it.
pub(crate) fn filename(media_type: &str) -> String {
    format!("audio.{}", crate::audio::format_of(media_type).unwrap_or("mp3"))
}

/// An answer (`json`, `verbose_json` or `diarized_json`).
pub(crate) fn parse_transcript(body: &Json, request: &TranscriptionRequest) -> Result<Transcript, LlmError> {
    let text = body["text"].as_str().ok_or_else(|| LlmError::new("transcription answer without `text`"))?;
    let mut segments: Vec<Segment> = body["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Segment {
            start: s["start"].as_f64().unwrap_or(0.0),
            end: s["end"].as_f64().unwrap_or(0.0),
            text: s["text"].as_str().unwrap_or_default().trim().to_string(),
            speaker: s["speaker"].as_str().map(String::from),
        })
        .collect();
    let duration = body["duration"].as_f64();
    if request.segments && segments.is_empty() && !text.trim().is_empty() {
        segments.push(Segment { start: 0.0, end: duration.unwrap_or(0.0), text: text.trim().into(), speaker: None });
    }
    let usage = &body["usage"];
    let tokens = |v: &Json| v.as_u64().unwrap_or(0);
    let (usage_tokens, seconds) = match usage["type"].as_str() {
        Some("tokens") => (
            Usage {
                input_tokens: tokens(&usage["input_tokens"]),
                output_tokens: tokens(&usage["output_tokens"]),
                ..Usage::default()
            },
            duration,
        ),
        Some("duration") => (Usage::default(), usage["seconds"].as_f64().or(duration)),
        _ => (Usage::default(), duration),
    };
    let language = body["language"].as_str().or_else(|| body["languages"][0]["code"].as_str()).map(String::from);
    Ok(Transcript {
        text: text.trim().to_string(),
        segments,
        language,
        usage: usage_tokens,
        seconds,
        model: request.model.name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request<'a>(model: &'a ModelConfig, segments: bool) -> TranscriptionRequest<'a> {
        TranscriptionRequest {
            model,
            media_type: "audio/mpeg".into(),
            data: "AAAA".into(),
            language: Some("fr".into()),
            prompt: None,
            keywords: Vec::new(),
            segments,
        }
    }

    fn openai() -> TranscriptionApi {
        crate::catalog::provider("openai").unwrap().transcription.unwrap()
    }

    #[test]
    fn each_model_gets_its_fields() {
        let whisper = ModelConfig::new("openai", "whisper-1");
        let fields = form_fields(&request(&whisper, true));
        assert_eq!(
            fields,
            [
                ("model", "whisper-1".to_string()),
                ("response_format", "verbose_json".into()),
                ("timestamp_granularities[]", "segment".into()),
                ("language", "fr".into()),
            ]
        );
        let diarize = ModelConfig::new("openai", "gpt-4o-transcribe-diarize");
        let fields = form_fields(&request(&diarize, true));
        assert_eq!(fields[1], ("response_format", "diarized_json".to_string()));
        assert_eq!(fields[2], ("chunking_strategy", "auto".to_string()));
        let transcribe = ModelConfig::new("openai", "gpt-transcribe");
        let mut asked = request(&transcribe, false);
        asked.prompt = Some("Ada, Grace".into());
        asked.keywords = vec!["Grenat".into()];
        let fields = form_fields(&asked);
        assert_eq!(fields[1], ("response_format", "json".to_string()));
        assert_eq!(
            &fields[2..],
            [("languages[]", "fr".to_string()), ("prompt", "Ada, Grace".into()), ("keywords[]", "Grenat".into())]
        );
    }

    #[test]
    fn what_a_model_cannot_do_is_said_before_uploading() {
        let api = openai();
        let mini = ModelConfig::new("openai", "gpt-4o-mini-transcribe");
        assert!(check(&request(&mini, false), &api).is_ok());
        let e = check(&request(&mini, true), &api).unwrap_err();
        assert!(e.message.starts_with("`gpt-4o-mini-transcribe` gives no timestamps"), "{}", e.message);
        let diarize = ModelConfig::new("openai", "gpt-4o-transcribe-diarize");
        let mut prompted = request(&diarize, true);
        prompted.prompt = Some("x".into());
        assert_eq!(check(&prompted, &api).unwrap_err().message, "`gpt-4o-transcribe-diarize` takes no `prompt:`");
        let whisper = ModelConfig::new("openai", "whisper-1");
        let mut keywords = request(&whisper, false);
        keywords.keywords = vec!["x".into()];
        assert!(check(&keywords, &api).unwrap_err().message.contains("no `keywords:`"));
        let mut aiff = request(&whisper, false);
        aiff.media_type = "audio/aiff".into();
        let e = check(&aiff, &api).unwrap_err();
        assert_eq!(e.message, "`whisper-1` transcribes flac, mp3, m4a, ogg, opus, wav, webm audio, not `audio/aiff`");
        // 25 MB and a byte: 26,214,401 bytes are 34,952,536 characters of base64
        let mut large = request(&whisper, false);
        large.data = format!("{}=", "A".repeat(34_952_535));
        let e = check(&large, &api).unwrap_err();
        assert!(
            e.message.starts_with("the audio is 26214401 bytes (25.0 MB): `whisper-1` takes 26214400 bytes at most"),
            "{}",
            e.message
        );
        // 25 MB exactly
        large.data = format!("{}==", "A".repeat(34_952_534));
        assert!(check(&large, &api).is_ok());
    }

    #[test]
    fn answers_are_read_with_their_segments_and_usage() {
        let whisper = ModelConfig::new("openai", "whisper-1");
        let verbose = json!({
            "task": "transcribe", "language": "french", "duration": 8.5, "text": " Bonjour. On commence. ",
            "segments": [
                {"id": 0, "seek": 0, "start": 0.0, "end": 2.0, "text": " Bonjour.", "tokens": [1]},
                {"id": 1, "seek": 0, "start": 2.0, "end": 8.5, "text": " On commence.", "tokens": [2]}
            ],
            "usage": {"type": "duration", "seconds": 9}
        });
        let t = parse_transcript(&verbose, &request(&whisper, true)).unwrap();
        assert_eq!(t.text, "Bonjour. On commence.");
        assert_eq!(t.segments[1], Segment { start: 2.0, end: 8.5, text: "On commence.".into(), speaker: None });
        assert_eq!((t.seconds, t.language.as_deref()), (Some(9.0), Some("french")));
        let diarized = json!({
            "text": "Hi. Hello.", "duration": 3.0,
            "segments": [{"type": "transcript.text.segment", "id": "seg_0", "start": 0.1, "end": 1.0, "text": "Hi.", "speaker": "A"}],
            "usage": {"type": "tokens", "input_tokens": 50, "output_tokens": 8, "total_tokens": 58, "input_token_details": {"audio_tokens": 50}}
        });
        let t = parse_transcript(&diarized, &request(&whisper, true)).unwrap();
        assert_eq!(t.segments[0].speaker.as_deref(), Some("A"));
        assert_eq!((t.usage.input_tokens, t.usage.output_tokens, t.seconds), (50, 8, Some(3.0)));
        let plain = json!({"text": "Bonjour", "languages": [{"code": "fr"}]});
        let t = parse_transcript(&plain, &request(&whisper, false)).unwrap();
        assert_eq!((t.language.as_deref(), t.seconds, t.segments.len()), (Some("fr"), None, 0));
        // segments asked of an answer without them: the whole text, one segment
        let t = parse_transcript(&plain, &request(&whisper, true)).unwrap();
        assert_eq!(t.segments, [Segment { start: 0.0, end: 0.0, text: "Bonjour".into(), speaker: None }]);
        assert!(parse_transcript(&json!({"error": {}}), &request(&whisper, false)).is_err());
        assert_eq!(filename("audio/mp4"), "audio.m4a");
    }
}
