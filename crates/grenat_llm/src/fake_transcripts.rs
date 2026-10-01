//! Transcripts for tests: given replies, in order (or one for every call),
//! shaped as each request asks — the text, or timed segments. A request is
//! checked as the provider would check it (its format, its size, the
//! options its model takes), so that a test fails where production would.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use crate::*;

/// One reply of a fake transcription model.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptReply {
    /// The text of the audio.
    Text(String),
    /// Its segments: the text is theirs, put together.
    Segments(Vec<Segment>),
    /// The call fails.
    Error(String),
}

pub struct FakeTranscripts {
    label: String,
    replies: Mutex<VecDeque<TranscriptReply>>,
    /// The reply of every call once `replies` are used up.
    always: Option<TranscriptReply>,
}

impl FakeTranscripts {
    /// `label` names the fake in errors (the model it stands for).
    pub fn new(
        label: impl Into<String>,
        replies: impl IntoIterator<Item = TranscriptReply>,
        always: Option<TranscriptReply>,
    ) -> FakeTranscripts {
        FakeTranscripts { label: label.into(), replies: Mutex::new(replies.into_iter().collect()), always }
    }

    fn next(&self) -> Option<TranscriptReply> {
        let next = self.replies.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
        next.or_else(|| self.always.clone())
    }
}

impl Provider for FakeTranscripts {
    fn complete(&self, _request: &Request) -> Result<Response, LlmError> {
        Err(LlmError::new(format!("the fake of {} only transcribes", self.label)))
    }

    fn transcribe(&self, request: &TranscriptionRequest) -> Result<Transcript, LlmError> {
        if let Some(api) = crate::catalog::provider(&request.model.provider).and_then(|p| p.transcription) {
            crate::transcription_wire::check(request, &api)?;
        }
        let segments = match self.next() {
            None => return Err(LlmError::new(format!("the fake of {} has no transcript left", self.label))),
            Some(TranscriptReply::Error(message)) => return Err(LlmError::new(message)),
            Some(TranscriptReply::Text(text)) => vec![Segment { start: 0.0, end: 0.0, text, speaker: None }],
            Some(TranscriptReply::Segments(segments)) => segments,
        };
        let text = segments.iter().map(|s| s.text.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ");
        Ok(Transcript {
            text,
            segments: if request.segments { segments } else { Vec::new() },
            language: request.language.clone(),
            usage: Usage::default(),
            seconds: None,
            model: "fake".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(model: &ModelConfig, segments: bool) -> TranscriptionRequest<'_> {
        TranscriptionRequest {
            model,
            media_type: "audio/mpeg".into(),
            data: "AAAA".into(),
            language: None,
            prompt: None,
            keywords: Vec::new(),
            segments,
        }
    }

    #[test]
    fn replies_in_order_then_the_one_for_every_call() {
        let whisper = ModelConfig::new("openai", "whisper-1");
        let said = |text: &str, start: f64| Segment { start, end: start + 1.0, text: text.into(), speaker: None };
        let fake = FakeTranscripts::new(
            "`:whisper`",
            [
                TranscriptReply::Segments(vec![said("Hello.", 0.0), said("Next point.", 1.0)]),
                TranscriptReply::Text("plain".into()),
                TranscriptReply::Error("rate limited".into()),
            ],
            Some(TranscriptReply::Text("again".into())),
        );
        let first = fake.transcribe(&request(&whisper, false)).unwrap();
        assert_eq!((first.text.as_str(), first.segments.len()), ("Hello. Next point.", 0));
        let second = fake.transcribe(&request(&whisper, true)).unwrap();
        assert_eq!(second.segments, [Segment { start: 0.0, end: 0.0, text: "plain".into(), speaker: None }]);
        assert_eq!(fake.transcribe(&request(&whisper, false)).unwrap_err().message, "rate limited");
        assert_eq!(fake.transcribe(&request(&whisper, false)).unwrap().text, "again");
        assert_eq!(fake.transcribe(&request(&whisper, false)).unwrap().text, "again");
        // checked as the provider would
        let mini = ModelConfig::new("openai", "gpt-4o-mini-transcribe");
        assert!(fake.transcribe(&request(&mini, true)).unwrap_err().message.contains("gives no timestamps"));
        let empty = FakeTranscripts::new("`:whisper`", [], None);
        assert_eq!(
            empty.transcribe(&request(&whisper, false)).unwrap_err().message,
            "the fake of `:whisper` has no transcript left"
        );
    }
}
