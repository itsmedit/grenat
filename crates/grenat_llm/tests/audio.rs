//! Audio: transcriptions uploaded to OpenAI's `/audio/transcriptions` as
//! `multipart/form-data`, and audio in prompts (`input_audio`, Chat
//! Completions) — played against a local server that reads the bytes sent.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grenat_llm::catalog::{self, Catalogued};
use grenat_llm::{Anthropic, ModelConfig, ModelKind, OpenAi, Provider, Request, Segment, TranscriptionRequest};
use serde_json::{Value as Json, json};

struct Received {
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// A server answering `replies` (status, body) in order; what it received.
fn serve(replies: Vec<(u16, Json)>) -> (String, Arc<Mutex<Vec<Received>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();
    std::thread::spawn(move || {
        for (status, reply) in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut headers = HashMap::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                headers.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
            let length: usize = headers.get("content-length").map_or(0, |l| l.parse().unwrap());
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let path = request_line.split_whitespace().nth(1).unwrap().to_string();
            log.lock().unwrap().push(Received { path, headers, body });
            let payload = reply.to_string();
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let mut stream = stream;
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (url, received)
}

/// The parts of a `multipart/form-data` body: (name, file name, content).
fn parts(content_type: &str, body: &[u8]) -> Vec<(String, Option<String>, Vec<u8>)> {
    let boundary = format!("--{}", content_type.split("boundary=").nth(1).unwrap());
    let text = body.iter().map(|b| *b as char).collect::<String>();
    assert!(text.ends_with(&format!("{boundary}--\r\n")), "the body ends with the closing boundary");
    text.split(&boundary)
        .filter(|part| part.starts_with("\r\n"))
        .map(|part| {
            let (head, content) = part[2..].split_once("\r\n\r\n").unwrap();
            let field = |name: &str| {
                head.split(&format!("{name}=\"")).nth(1).map(|rest| rest.split('"').next().unwrap().to_string())
            };
            let content = content.strip_suffix("\r\n").unwrap();
            (field("name").unwrap(), field("filename"), content.chars().map(|c| c as u8).collect())
        })
        .collect()
}

fn model(name: &str) -> ModelConfig {
    ModelConfig { kind: ModelKind::Transcription, ..ModelConfig::new("openai", name) }
}

fn openai(url: &str) -> OpenAi {
    OpenAi::new(*catalog::provider("openai").unwrap(), Some("sk-test".into()), Some(url))
        .with_retry_delay(Duration::from_millis(1))
}

fn transcription<'a>(model: &'a ModelConfig, data: &str, segments: bool) -> TranscriptionRequest<'a> {
    TranscriptionRequest {
        model,
        media_type: "audio/mpeg".into(),
        data: data.into(),
        language: Some("fr".into()),
        prompt: Some("Participants: Ada, Grace.".into()),
        keywords: Vec::new(),
        segments,
    }
}

#[test]
fn a_transcription_uploads_the_file_and_reads_its_segments() {
    let answer = json!({
        "task": "transcribe", "language": "french", "duration": 7.5, "text": "Bonjour à tous. Premier point : le budget.",
        "segments": [
            {"id": 0, "seek": 0, "start": 0.0, "end": 2.5, "text": " Bonjour à tous.", "tokens": [1], "temperature": 0.0},
            {"id": 1, "seek": 0, "start": 2.5, "end": 7.5, "text": " Premier point : le budget.", "tokens": [2], "temperature": 0.0}
        ],
        "usage": {"type": "duration", "seconds": 8}
    });
    // a server error first: retried
    let (url, received) = serve(vec![(500, json!({"error": {"message": "busy"}})), (200, answer)]);
    let whisper = model("whisper-1");
    // "ID3" and three bytes no text encoding keeps
    let transcript = openai(&url).transcribe(&transcription(&whisper, "SUQzAP/+", true)).unwrap();
    assert_eq!(transcript.text, "Bonjour à tous. Premier point : le budget.");
    assert_eq!(
        transcript.segments,
        [
            Segment { start: 0.0, end: 2.5, text: "Bonjour à tous.".into(), speaker: None },
            Segment { start: 2.5, end: 7.5, text: "Premier point : le budget.".into(), speaker: None },
        ]
    );
    assert_eq!(transcript.seconds, Some(8.0));
    let received = received.lock().unwrap();
    assert_eq!(received.len(), 2);
    let last = &received[1];
    assert_eq!(last.path, "/v1/audio/transcriptions");
    assert_eq!(last.headers["authorization"], "Bearer sk-test");
    let content_type = &last.headers["content-type"];
    assert!(content_type.starts_with("multipart/form-data; boundary="), "{content_type}");
    let parts = parts(content_type, &last.body);
    let fields: Vec<(&str, String)> = parts
        .iter()
        .filter(|(_, file, _)| file.is_none())
        .map(|(name, _, content)| (name.as_str(), String::from_utf8(content.clone()).unwrap()))
        .collect();
    assert_eq!(
        fields,
        [
            ("model", "whisper-1".to_string()),
            ("response_format", "verbose_json".into()),
            ("timestamp_granularities[]", "segment".into()),
            ("language", "fr".into()),
            ("prompt", "Participants: Ada, Grace.".into()),
        ]
    );
    let (name, filename, content) = parts.last().unwrap();
    assert_eq!((name.as_str(), filename.as_deref()), ("file", Some("audio.mp3")));
    assert_eq!(content, &[b'I', b'D', b'3', 0x00, 0xff, 0xfe]);
    assert!(String::from_utf8_lossy(&last.body).contains("Content-Type: audio/mpeg\r\n"));
}

#[test]
fn token_usage_and_client_errors() {
    let answer =
        json!({"text": "Hi.", "usage": {"type": "tokens", "input_tokens": 40, "output_tokens": 3, "total_tokens": 43}});
    let (url, received) = serve(vec![(200, answer), (400, json!({"error": {"message": "Invalid file format."}}))]);
    let mini = model("gpt-4o-mini-transcribe");
    let mut request = transcription(&mini, "AAAA", false);
    request.prompt = None;
    let transcript = openai(&url).transcribe(&request).unwrap();
    assert_eq!((transcript.usage.input_tokens, transcript.usage.output_tokens), (40, 3));
    assert!((grenat_llm::transcription_cost("gpt-4o-mini-transcribe", &transcript).unwrap() - 0.000065).abs() < 1e-12);
    // a client error is not retried
    let e = openai(&url).transcribe(&request).unwrap_err();
    assert_eq!(e.message, "HTTP 400: Invalid file format.");
    assert_eq!(received.lock().unwrap().len(), 2);
}

#[test]
fn too_large_a_file_is_refused_before_anything_is_sent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let whisper = model("whisper-1");
    // 25 MB and a byte
    let data = format!("{}=", "A".repeat(34_952_535));
    let e = openai(&url).transcribe(&transcription(&whisper, &data, false)).unwrap_err();
    assert!(
        e.message.starts_with("the audio is 26214401 bytes (25.0 MB): `whisper-1` takes 26214400"),
        "{}",
        e.message
    );
    let mut wav = transcription(&whisper, "AAAA", false);
    wav.media_type = "audio/aiff".into();
    assert!(openai(&url).transcribe(&wav).unwrap_err().message.contains("not `audio/aiff`"));
    listener.set_nonblocking(true).unwrap();
    assert!(listener.accept().is_err(), "nothing was sent");
    // a provider without a transcription API
    let groq = OpenAi::new(*catalog::provider("groq").unwrap(), Some("k".into()), Some(&url));
    let e = groq.transcribe(&transcription(&whisper, "AAAA", false)).unwrap_err();
    assert!(e.message.starts_with("the provider `groq` makes no transcriptions in Grenat"), "{}", e.message);
}

fn with_audio<'a>(model: &'a ModelConfig, media_type: &str, data: &str) -> Request<'a> {
    Request {
        model,
        system: None,
        messages: vec![json!({"role": "user", "content": [
            {"type": "audio", "source": {"type": "base64", "media_type": media_type, "data": data}},
            {"type": "text", "text": "Summarize the meeting."}
        ]})],
        tools: Vec::new(),
        output_schema: None,
    }
}

#[test]
fn openai_takes_audio_through_chat_completions() {
    let answer = json!({
        "model": "gpt-audio",
        "choices": [{"message": {"role": "assistant", "content": "A short meeting."}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 120, "completion_tokens": 5}
    });
    let (url, received) = serve(vec![(200, answer)]);
    let audio_model = ModelConfig::new("openai", "gpt-audio");
    let response = openai(&url).complete(&with_audio(&audio_model, "audio/wav", "UklGRg==")).unwrap();
    assert_eq!(response.text(), "A short meeting.");
    let received = received.lock().unwrap();
    assert_eq!(received[0].path, "/v1/chat/completions");
    let body: Json = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({"type": "input_audio", "input_audio": {"data": "UklGRg==", "format": "wav"}})
    );
    assert_eq!(body["max_completion_tokens"], 16_000);
    assert!(body.get("max_output_tokens").is_none());
    // its formats only
    let e = openai(&url).complete(&with_audio(&audio_model, "audio/mp4", "AAAA")).unwrap_err();
    assert_eq!(
        e.message,
        "the provider `openai` takes wav, mp3 audio in prompts, not `audio/mp4`: convert it, or transcribe it first"
    );
}

#[test]
fn gemini_takes_audio_up_to_its_request_size() {
    let gemini = *catalog::provider("gemini").unwrap();
    let model = ModelConfig::new("gemini", "gemini-3-flash");
    let body = grenat_llm::chat_body(&with_audio(&model, "audio/mpeg", "SUQz"), &gemini).unwrap();
    assert_eq!(body["messages"][0]["content"][0]["input_audio"], json!({"data": "SUQz", "format": "mp3"}));
    let large = "A".repeat(20_000_004);
    let e = grenat_llm::chat_body(&with_audio(&model, "audio/mpeg", &large), &gemini).unwrap_err();
    assert!(
        e.message.starts_with("the audio is 19.1 MB once encoded: the provider `gemini` takes 19.1 MB"),
        "{}",
        e.message
    );
}

#[test]
fn providers_without_audio_say_to_transcribe_first() {
    let mistral: Catalogued = *catalog::provider("mistral").unwrap();
    let model = ModelConfig::new("mistral", "mistral-large");
    let e = grenat_llm::chat_body(&with_audio(&model, "audio/mpeg", "AAAA"), &mistral).unwrap_err();
    assert_eq!(
        e.message,
        "the provider `mistral` takes no audio in prompts in Grenat (those that do: openai, gemini): transcribe it first (`transcribe(:whisper, audio)`) and give the model its text"
    );
    // Anthropic: refused before anything is sent
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let claude = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let e = Anthropic::new("k", url).complete(&with_audio(&claude, "audio/mpeg", "AAAA")).unwrap_err();
    assert!(e.message.starts_with("Anthropic's models take no audio: transcribe it first"), "{}", e.message);
    listener.set_nonblocking(true).unwrap();
    assert!(listener.accept().is_err(), "nothing was sent");
}
