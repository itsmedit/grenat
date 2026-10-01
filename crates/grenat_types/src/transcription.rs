//! Transcriptions, checked: `transcribe(:whisper, audio)` names a
//! transcription model, takes an `Attachment` (`Audio.read`, `Audio.url`)
//! and gives `~String` — what was said is untrusted; with `segments: true`,
//! an `Array(TranscriptSegment)` whose `text` and `speaker` are. It is an
//! `llm` effect no secret reaches.

use grenat_ast::{Diagnostic, ModelDecl, Span};

use crate::ty::{Ty, V};
use crate::*;

/// The options of `transcribe`.
const OPTIONS: [&str; 4] = ["language", "prompt", "keywords", "segments"];

/// Whether a model declaration says `kind: :transcription`.
pub(crate) fn declares_transcription(model: &ModelDecl) -> bool {
    crate::embeddings::declares_kind(model, "transcription")
}

impl<'p> Checker<'p> {
    /// `transcribe(:whisper, audio, language: "fr", segments: true)`, `transcribe(audio)`.
    pub(crate) fn transcribe_call(&mut self, cx: &mut Ctx<'p>, span: Span, argv: &[ArgV]) -> V {
        self.secrets_to_model(argv, "transcribe");
        cx.add_effect(Eff { path: "llm".into(), arg: None, origin: span });
        let positional: Vec<&ArgV> = argv.iter().filter(|a| a.name.is_none()).collect();
        let audio = match positional.as_slice() {
            [model, audio] => {
                self.transcription_model_ref(model);
                audio
            }
            [audio] => {
                if self.transcription_models.is_empty() {
                    self.report(Diagnostic::new(span, "no transcription model declared").with_code(E_DECL).with_help(
                        "add `model :whisper, provider: :openai, name: \"whisper-1\", kind: :transcription`",
                    ));
                }
                audio
            }
            _ => {
                self.error(E_TYPE, span, "`transcribe` expects a model and audio: `transcribe(:whisper, audio)`");
                return V::unknown();
            }
        };
        if !self.compat(&audio.v.ty, &Ty::User(builtins::ATTACHMENT.into())) {
            let message = format!("`transcribe` expects audio (`Audio.read(path)`), got `{}`", audio.v.ty);
            self.error(E_TYPE, audio.span, message);
        }
        let mut segments = Some(false);
        for arg in argv {
            match arg.name.as_deref() {
                Some("segments") => segments = arg.flag,
                Some(name) if !OPTIONS.contains(&name) => {
                    let message = format!("unknown named argument `{name}:` for `transcribe`");
                    self.error_help(E_TYPE, arg.span, message, suggest(name, OPTIONS));
                }
                _ => {}
            }
        }
        match segments {
            Some(false) => V::new(Ty::Str).tainted(Some(span)),
            // what each segment says is untrusted (its fields are)
            Some(true) => V::new(Ty::array(Ty::User(builtins::TRANSCRIPT_SEGMENT.into()))),
            // not a literal: either
            None => V::unknown(),
        }
    }

    /// The model of `transcribe`: declared, and a transcription model.
    fn transcription_model_ref(&mut self, model: &ArgV) {
        let Some(name) = model.lit.as_deref().filter(|_| model.v.ty == Ty::Sym) else {
            if !model.v.ty.is_unknown() {
                self.error(E_TYPE, model.span, "`transcribe` expects a model first: `transcribe(:whisper, audio)`");
            }
            return;
        };
        if !self.models.contains(&name) {
            let models = self.transcription_models.clone();
            self.error_help(E_NAME, model.span, format!("model `:{name}` is not declared"), suggest(name, models));
        } else if !self.transcription_models.contains(&name) {
            self.report(
                Diagnostic::new(model.span, format!("model `:{name}` is not a transcription model"))
                    .with_code(E_TYPE)
                    .with_help("declare it with `kind: :transcription`"),
            );
        }
    }
}
