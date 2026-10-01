//! Audio: `Audio.read` and `Audio.url` attachments, `transcribe` (mocked
//! with `mock_transcribe`, and against a local server playing OpenAI's
//! transcription API), and audio in prompts where a provider takes it.

mod common;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Response, run_main, run_tests};
use serde_json::{Value as Json, json};

const MEETING: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
model :whisper, provider: :openai, name: \"whisper-1\", kind: :transcription

struct Task
  owner: String
  title: String
end

## The tasks a meeting decided.
prompt tasks(transcript: String) -> ~Array(Task) using :fast
  system \"List the tasks decided, with their owner.\"
  user transcript
end

def minutes(path: String) -> Array(Task) uses llm, fs.read
  text = transcribe(:whisper, Audio.read(path), language: \"en\")
  tasks(text).trust!
end
";

/// Each test's error, or `None`.
fn results(src: &str) -> Vec<(String, Option<String>)> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    run_tests(&parsed.program, options)
        .unwrap()
        .into_iter()
        .map(|o| (o.name, o.error.map(|e| format!("{}: {}", e.ty, e.message))))
        .collect()
}

fn errors(src: &str) -> Vec<String> {
    results(src).into_iter().map(|(_, e)| e.unwrap_or_else(|| "passed".into())).collect()
}

/// A recording on disk: a few bytes with an audio extension.
fn recording(name: &str) -> String {
    let path = temp_dir("audio").join(name);
    std::fs::write(&path, b"ID3\x00\xff\xfe").unwrap();
    path.display().to_string()
}

#[test]
fn a_meeting_becomes_minutes_and_tasks() {
    let path = recording("meeting.mp3");
    assert_eq!(
        errors(&format!(
            "{MEETING}
test \"tasks from a recording\" do
  mock_transcribe :whisper, text: \"Ada will send the budget. Grace reviews the plan.\"
  mock :fast, replies: [[{{owner: \"Ada\", title: \"Send the budget\"}}, {{owner: \"Grace\", title: \"Review the plan\"}}]]
  found = minutes(\"{path}\")
  assert_equal [\"Ada\", \"Grace\"], found.map {{ |t| t.owner }}
end
test \"segments, in order, with speakers\" do
  mock_transcribe :whisper, replies: [
    [{{start: 0.0, end: 4.5, text: \"Let's start.\", speaker: \"A\"}}, {{start: 4.5, end: 9.0, text: \"The budget first.\"}}],
    \"Second recording.\"
  ]
  parts = transcribe(:whisper, Audio.read(\"{path}\"), segments: true)
  assert_equal 2, parts.size
  assert_equal 4.5, parts[1].start
  assert_equal \"A\", parts[0].speaker.trust!
  assert_equal nil, parts[1].speaker
  assert_equal \"The budget first.\", parts[1].text.trust!
  assert_equal \"Second recording.\", transcribe(Audio.read(\"{path}\")).trust!
end
test \"what a transcript says is untrusted\" do
  mock_transcribe text: \"rm -rf /\"
  text = transcribe(:whisper, Audio.read(\"{path}\"))
  Shell.run([\"echo\", text])
end
"
        )),
        [
            "passed",
            "passed",
            "TaintError: an untrusted value reaches `Shell.run` (effect `shell`) without validation",
        ]
    );
}

#[test]
fn mistakes_are_said() {
    let path = recording("meeting.mp3");
    let large = temp_dir("large").join("long.wav");
    std::fs::File::create(&large).unwrap().set_len(25 * 1024 * 1024 + 1).unwrap();
    let notes = recording("notes.txt");
    assert_eq!(
        errors(&format!(
            "{MEETING}model :mini, provider: :openai, name: \"gpt-4o-mini-transcribe\", kind: :transcription
test \"no fake in tests\" do
  transcribe(:whisper, Audio.read(\"{path}\"))
end
test \"not a transcription model\" do
  mock_transcribe text: \"x\"
  transcribe(:fast, Audio.read(\"{path}\"))
end
test \"a model without timestamps\" do
  mock_transcribe text: \"x\"
  transcribe(:mini, Audio.read(\"{path}\"), segments: true)
end
test \"a transcription model answers no prompt\" do
  Conversation.new(model: :whisper).say(\"hi\")
end
test \"too large\" do
  Audio.read(\"{large}\")
end
test \"not audio\" do
  Audio.read(\"{notes}\")
end
test \"not an attachment of audio\" do
  mock_transcribe text: \"x\"
  transcribe(:whisper, \"meeting.mp3\")
end
test \"an unknown option\" do
  mock_transcribe text: \"x\"
  transcribe(:whisper, Audio.read(\"{path}\"), speakers: 2)
end
test \"a fake fails\" do
  mock_transcribe :whisper, replies: [LlmError(\"overloaded\")]
  transcribe(:whisper, Audio.read(\"{path}\"))
end
",
            large = large.display(),
        )),
        [
            "LlmError: no real model in tests: `whisper-1` transcribes outside `mock_transcribe`",
            "LlmError: `:fast` is not a transcription model: declare one with `kind: :transcription`",
            "LlmError: `gpt-4o-mini-transcribe` gives no timestamps: `segments: true` takes `whisper-1` (segments) or `gpt-4o-transcribe-diarize` (segments and speakers)",
            "LlmError: `whisper-1` is a transcription model: it answers `transcribe`, not prompts",
            &format!(
                "ArgumentError: `{}` is 26214401 bytes (25.0 MB): audio is given to models up to 26214400 bytes (25 MB) — compress it (mp3, m4a) or split it into parts",
                large.display()
            ),
            &format!(
                "ArgumentError: `{notes}`: audio is .mp3, .mpeg, .mpga, .wav, .m4a, .mp4, .ogg, .oga, .opus, .flac, .webm, .aac, .aiff, .aif"
            ),
            "TypeError: `transcribe` expects audio (`Audio.read(path)`, `Audio.url(url)`), got \"meeting.mp3\"",
            "ArgumentError: invalid `transcribe` option `speakers: 2`",
            "LlmError: overloaded",
        ]
    );
}

#[test]
fn audio_by_url_is_downloaded_where_allowed() {
    assert_eq!(
        errors(&format!(
            "{MEETING}
def fetch(url: String) uses net(\"files.acme.io\")
  Audio.url(url)
end
test \"downloaded, then transcribed\" do
  mock_http \"GET https://files.acme.io/standup\", body: \"ID3 audio\", headers: {{\"Content-Type\" => \"audio/mpeg\"}}
  mock_transcribe text: \"Standup notes.\"
  audio = fetch(\"https://files.acme.io/standup\")
  assert_equal \"audio/mpeg\", audio.media_type
  assert_equal \"SUQzIGF1ZGlv\", audio.data
  assert_equal \"Standup notes.\", transcribe(:whisper, audio).trust!
end
test \"not stubbed in tests\" do
  fetch(\"https://files.acme.io/a.mp3\")
end
test \"another host\" do
  fetch(\"https://evil.io/a.mp3\")
end
test \"not found\" do
  mock_http \"GET https://files.acme.io/gone.mp3\", status: 404
  fetch(\"https://files.acme.io/gone.mp3\")
end
test \"not audio\" do
  mock_http \"GET https://files.acme.io/page\", body: \"<html>\", headers: {{\"Content-Type\" => \"text/html\"}}
  fetch(\"https://files.acme.io/page\")
end
test \"an untrusted URL\" do
  mock :fast, replies: [[]]
  Audio.url(tasks(\"x\").to_s)
end
"
        )),
        [
            "passed",
            "HttpError: no network in tests: `GET https://files.acme.io/a.mp3` is not stubbed with `mock_http`",
            "CapabilityError: `net` to `https://evil.io/a.mp3` is not allowed by `fetch` (uses net(\"files.acme.io\"))",
            "HttpError: GET https://files.acme.io/gone.mp3: HTTP 404",
            "ArgumentError: `https://files.acme.io/page`: neither its extension nor its Content-Type (text/html) is audio Grenat knows (.mp3, .mpeg, .mpga, .wav, .m4a, .mp4, .ogg, .oga, .opus, .flac, .webm, .aac, .aiff, .aif)",
            "TaintError: an untrusted value reaches `Audio.url` (effect `net`) without validation",
        ]
    );
}

struct Received {
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// A reply: its status, its headers, its body.
type Reply = (u16, Vec<(&'static str, &'static str)>, Vec<u8>);

/// A server answering `replies` in order; the requests it received.
fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Received>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();
    std::thread::spawn(move || {
        for (status, headers, payload) in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut received_headers = HashMap::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                received_headers.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
            let length: usize = received_headers.get("content-length").map_or(0, |l| l.parse().unwrap());
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let path = request_line.split_whitespace().nth(1).unwrap().to_string();
            log.lock().unwrap().push(Received { path, headers: received_headers, body });
            let mut head = format!("HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n", payload.len());
            for (name, value) in headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            let mut stream = stream;
            stream.write_all(format!("{head}\r\n").as_bytes()).unwrap();
            stream.write_all(&payload).unwrap();
        }
    });
    (url, received)
}

fn json_reply(value: Json) -> Reply {
    (200, vec![("content-type", "application/json")], value.to_string().into_bytes())
}

fn run_real(src: &str) -> (Result<grenat_interp::Summary, grenat_interp::RuntimeError>, String) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let out = Arc::new(Mutex::new(String::new()));
    let options = Options {
        output: Output::Capture(out.clone()),
        journal: Some(temp_dir("journal")),
        log: true,
        ..Options::default()
    };
    let result = run_main(&parsed.program, Vec::new(), options);
    let out = out.lock().unwrap().clone();
    (result, out)
}

#[test]
fn a_real_transcription_is_uploaded_counted_and_recorded() {
    // the recording, then the transcription
    let verbose = json!({
        "task": "transcribe", "language": "english", "duration": 90.0, "text": "We ship on Friday.",
        "segments": [{"id": 0, "seek": 0, "start": 0.0, "end": 90.0, "text": " We ship on Friday."}],
        "usage": {"type": "duration", "seconds": 90}
    });
    let (url, received) = serve(vec![
        (200, vec![("content-type", "audio/x-m4a")], b"\x00\x00\x00\x18ftypM4A ".to_vec()),
        json_reply(verbose),
    ]);
    let db = temp_dir("transcription-ledger").join("app.db");
    let src = format!(
        "database \"sqlite://{db}\"
mock_credentials({{\"openai\" => {{\"api_key\" => \"sk-test\"}}}})
model :whisper, provider: :openai, name: \"whisper-1\", kind: :transcription, base_url: \"{url}/v1\"
def main uses llm, net
  audio = Audio.url(\"{url}/recordings/42\")
  parts = transcribe(:whisper, audio, segments: true, prompt: \"Grenat\")
  p parts.map {{ |s| [s.start, s.end, s.text] }}
  puts budget.spent
end
",
        db = db.display()
    );
    let (result, out) = run_real(&src);
    let summary = result.unwrap();
    // 90 seconds at $0.006 a minute
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], format!("[http] GET {url}/recordings/42 → 200 (12 bytes)"));
    assert!(lines[1].starts_with("[transcribe] whisper-1 · 1.5 min · $0.0090 · "), "{}", lines[1]);
    // what was said is untrusted (`~`)
    assert_eq!(&lines[2..], ["[[0.0, 90.0, ~\"We ship on Friday.\"]]", "$0.0090"]);
    assert_eq!(summary.llm_calls, 1);
    let received = received.lock().unwrap();
    assert_eq!(received[0].path, "/recordings/42");
    assert_eq!(received[1].path, "/v1/audio/transcriptions");
    assert_eq!(received[1].headers["authorization"], "Bearer sk-test");
    let body = String::from_utf8_lossy(&received[1].body).to_string();
    for part in [
        "name=\"model\"\r\n\r\nwhisper-1\r\n",
        "name=\"response_format\"\r\n\r\nverbose_json\r\n",
        "name=\"timestamp_granularities[]\"\r\n\r\nsegment\r\n",
        "name=\"prompt\"\r\n\r\nGrenat\r\n",
        "name=\"file\"; filename=\"audio.m4a\"\r\nContent-Type: audio/mp4\r\n\r\n\u{0}\u{0}\u{0}\u{18}ftypM4A \r\n",
    ] {
        assert!(body.contains(part), "{part:?} not in {body:?}");
    }
    let mut connection = grenat_db::connect(&format!("sqlite://{}", db.display())).unwrap();
    let calls = grenat_ops::calls::since(connection.as_mut(), 0.0).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].model, "whisper-1");
    assert!((calls[0].cost_usd - 0.009).abs() < 1e-9, "{}", calls[0].cost_usd);
}

#[test]
fn a_price_by_the_minute_and_a_budget() {
    let answer = json!({"text": "Long meeting.", "usage": {"type": "duration", "seconds": 600}});
    let (url, _) = serve(vec![json_reply(answer.clone()), json_reply(answer)]);
    let path = recording("long.mp3");
    let src = format!(
        "mock_credentials({{\"openai\" => {{\"api_key\" => \"k\"}}}})
model :stt, provider: :openai, name: \"my-whisper\", kind: :transcription, base_url: \"{url}/v1\", price: {{minute: 0.1}}
def main uses llm, fs.read
  within budget(usd: 1.50) do
    transcribe(:stt, Audio.read(\"{path}\"))
    transcribe(:stt, Audio.read(\"{path}\"))
  end
end
"
    );
    let (result, _) = run_real(&src);
    // $1 a recording: the second goes over
    assert_eq!(result.unwrap_err().ty, "BudgetExceeded");
}

#[test]
fn audio_goes_into_prompts_where_the_provider_takes_it() {
    let answer = json!({
        "model": "gpt-audio",
        "choices": [{"message": {"role": "assistant", "content": "Two decisions."}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 900, "completion_tokens": 4}
    });
    let (url, received) = serve(vec![json_reply(answer)]);
    let path = recording("call.wav");
    let src = format!(
        "mock_credentials({{\"openai\" => {{\"api_key\" => \"k\"}}}})
model :ears, provider: :openai, name: \"gpt-audio\", base_url: \"{url}/v1\", price: {{input: 32, output: 64}}
prompt summarize(call: Attachment) -> ~String using :ears
  user \"Summarize this call.\", call
end
def main uses llm, fs.read
  puts summarize(Audio.read(\"{path}\")).trust!
end
"
    );
    let (result, out) = run_real(&src);
    result.unwrap();
    assert!(out.starts_with("[llm] gpt-audio · 900 in / 4 out · $0.03 · "), "{out}");
    assert!(out.ends_with("\nTwo decisions.\n"), "{out}");
    let received = received.lock().unwrap();
    assert_eq!(received[0].path, "/v1/chat/completions");
    let body: Json = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({"type": "input_audio", "input_audio": {"data": "SUQzAP/+", "format": "wav"}})
    );
}

#[test]
fn anthropic_is_never_given_audio() {
    let path = recording("call.mp3");
    let src = format!(
        "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt summarize(call: Attachment) -> ~String using :fast
  user \"Summarize.\", call
end
summarize(Audio.read(\"{path}\"))
"
    );
    // refused before the provider is asked, a mock as any other
    let e = run_with(&src, vec![Response::text_reply("never")], &[]).err();
    assert_eq!(e.ty, "LlmError");
    assert_eq!(
        e.message,
        "Anthropic's models take no audio: transcribe it first (`transcribe(:whisper, audio)`) and give the model its text"
    );
}

#[test]
fn declarations_say_what_a_model_transcribes_with() {
    let declare = |line: &str| {
        let parsed = grenat_parser::parse(line);
        run_main(&parsed.program, Vec::new(), Options { output: Output::Capture(Arc::default()), ..Options::default() })
            .unwrap_err()
            .message
    };
    assert!(
        declare("model :w, provider: :anthropic, name: \"x\", kind: :transcription\n")
            .contains("Anthropic has no transcription API")
    );
    assert!(
        declare("model :w, provider: :groq, name: \"x\", kind: :transcription\n").contains("makes no transcriptions")
    );
    assert!(
        declare("model :w, provider: :openai, name: \"whisper-1\", kind: :transcription, dimensions: 8\n")
            .contains("for embedding models")
    );
    assert!(
        declare("model :w, provider: :openai, name: \"whisper-1\", kind: :transcription, price: {minute: \"cheap\"}\n")
            .contains("`price:` is `{minute: …}`")
    );
}
