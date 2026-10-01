//! Audio, checked: `Audio.read` and `Audio.url` (their effects),
//! transcription models, `transcribe` and its segments (E0100, E0200,
//! E0300, E0412, E0414, E0500).

mod common;

use common::*;

const MODELS: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
model :whisper, provider: :openai, name: \"whisper-1\", kind: :transcription, price: {minute: 0.006}
";

#[test]
fn transcripts_are_typed_and_untrusted() {
    clean(&format!(
        "{MODELS}
struct Task
  owner: String
  title: String
end
prompt tasks(transcript: String) -> ~Array(Task) using :fast
  user transcript
end
def minutes(path: String) -> Array(Task) uses llm, fs.read
  text = transcribe(:whisper, Audio.read(path), language: \"fr\", prompt: \"Ada, Grace\")
  tasks(text).trust!
end
def timeline(audio: Attachment) -> Array(TranscriptSegment) uses llm = transcribe(audio, segments: true)
def starts(audio: Attachment) -> Array(Float) uses llm
  timeline(audio).map {{ |s| s.end - s.start }}
end
def said(audio: Attachment) -> String uses llm
  timeline(audio).map {{ |s| s.text.trust! }}.join(\" \")
end
def fetch(url: String) -> Attachment uses net(\"files.acme.io\") = Audio.url(\"https://files.acme.io/a.mp3\")
test \"fake transcripts\" do
  mock_transcribe :whisper, text: \"x\"
  mock_transcribe replies: [\"a\", [{{start: 0.0, end: 1.0, text: \"b\"}}]]
end
"
    ));
}

#[test]
fn what_was_said_cannot_reach_a_command_unchecked() {
    single(
        &format!(
            "{MODELS}def f(a: Attachment) uses llm, shell\n  Shell.run([\"echo\", transcribe(:whisper, a)])\nend\n"
        ),
        "E0412",
        "[\"echo\", transcribe(:whisper, a)]",
    );
    single(
        &format!(
            "{MODELS}def f(a: Attachment) uses llm, shell\n  Shell.run([\"echo\", transcribe(:whisper, a, segments: true).first&.text])\nend\n"
        ),
        "E0412",
        "[\"echo\", transcribe(:whisper, a, segments: true).first&.text]",
    );
}

#[test]
fn effects_and_secrets() {
    single(
        &format!("{MODELS}def f(a: Attachment) -> String uses db\n  transcribe(:whisper, a).trust!\nend\n"),
        "E0300",
        "transcribe(:whisper, a)",
    );
    single("def f -> Attachment uses llm\n  Audio.read(\"a.mp3\")\nend\n", "E0300", "Audio.read(\"a.mp3\")");
    single(
        "def f -> Attachment uses net(\"files.acme.io\")\n  Audio.url(\"https://evil.io/a.mp3\")\nend\n",
        "E0300",
        "Audio.url(\"https://evil.io/a.mp3\")",
    );
    single(
        &format!(
            "{MODELS}def f(a: Attachment) uses llm, env\n  transcribe(:whisper, a, prompt: Credentials.fetch(:x, :y))\nend\n"
        ),
        "E0414",
        "Credentials.fetch(:x, :y)",
    );
}

#[test]
fn transcribe_names_a_transcription_model_and_takes_audio() {
    single(&format!("{MODELS}transcribe(:fast, Audio.read(\"a.mp3\"))\n"), "E0200", ":fast");
    single(&format!("{MODELS}transcribe(:whispr, Audio.read(\"a.mp3\"))\n"), "E0100", ":whispr");
    single(&format!("{MODELS}transcribe(:whisper, \"a.mp3\")\n"), "E0200", "\"a.mp3\"");
    single(&format!("{MODELS}transcribe(:whisper, Audio.read(\"a.mp3\"), speakers: 2)\n"), "E0200", "2");
    single(&format!("{MODELS}transcribe()\n"), "E0200", "transcribe()");
    let without = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\ntranscribe(Audio.read(\"a.mp3\"))\n";
    single(without, "E0500", "transcribe(Audio.read(\"a.mp3\"))");
}

#[test]
fn models_are_declared_for_what_their_provider_does() {
    single("model :w, provider: :anthropic, name: \"x\", kind: :transcription\n", "E0500", ":anthropic");
    single("model :w, provider: :mistral, name: \"x\", kind: :transcription\n", "E0500", ":mistral");
    single("model :w, provider: :openai, name: \"x\", kind: :speech\n", "E0500", ":speech");
    single(
        "model :w, provider: :openai, name: \"whisper-1\", kind: :transcription, dimensions: 8\n",
        "E0500",
        "dimensions:",
    );
    // a prompt answers with a chat model: a transcription model is not the default
    let only_whisper = "model :w, provider: :openai, name: \"whisper-1\", kind: :transcription\n";
    single(&format!("{only_whisper}prompt p(q: String) -> ~String\n  user q\nend\n"), "E0500", "p");
    let d =
        single(&format!("{MODELS}prompt p(q: String) -> ~String using :whisper\n  user q\nend\n"), "E0500", ":whisper");
    assert!(d.message.contains("transcription model"), "{}", d.message);
}

#[test]
fn a_program_cannot_declare_an_attachment_of_its_own() {
    let d = single(
        "struct Attachment\n  kind: String\n  media_type: String\n  source: String\n  data: String\nend\n",
        "E0100",
        "Attachment",
    );
    assert_eq!(d.message, "type `Attachment` is built in: give yours another name");
    single("struct TranscriptSegment\n  text: String\nend\n", "E0100", "TranscriptSegment");
}
