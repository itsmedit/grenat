//! The OpenAI connector — the Responses API for OpenAI itself, Chat
//! Completions for the providers speaking its protocol — tested against a
//! local server that plays the API (and once against OpenAI itself: see
//! SPEC, phase 10).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grenat_llm::catalog::{self, Catalogued};
use grenat_llm::{ModelConfig, OpenAi, Provider, Request, ToolSpec, chat_body};
use serde_json::{Value as Json, json};

struct Received {
    path: String,
    headers: HashMap<String, String>,
    body: Json,
}

/// Starts a server that answers with `replies` (status, body), in order.
fn serve(replies: Vec<(u16, Json)>) -> (String, Arc<Mutex<Vec<Received>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();
    std::thread::spawn(move || {
        for (status, body) in replies {
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
            let mut raw = vec![0; length];
            reader.read_exact(&mut raw).unwrap();
            log.lock().unwrap().push(Received {
                path: request_line.split_whitespace().nth(1).unwrap().to_string(),
                headers,
                body: serde_json::from_slice(&raw).unwrap_or(Json::Null),
            });
            let payload = match &body {
                Json::String(text) => text.clone(),
                other => other.to_string(),
            };
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

fn provider(name: &str) -> Catalogued {
    *catalog::provider(name).unwrap()
}

/// A provider speaking Chat Completions with every feature (as OpenAI's own
/// Chat Completions does).
fn chat() -> Catalogued {
    Catalogued {
        name: "chat",
        protocol: catalog::Protocol::OpenAi,
        documents: true,
        max_tokens_field: "max_completion_tokens",
        ..provider("openai")
    }
}

fn answer(message: Json, finish: &str) -> Json {
    json!({
        "model": "gpt-5-2026-08-01",
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": {"prompt_tokens": 120, "completion_tokens": 7, "prompt_tokens_details": {"cached_tokens": 20}}
    })
}

fn request<'a>(model: &'a ModelConfig, messages: Vec<Json>) -> Request<'a> {
    Request {
        model,
        system: Some("be brief".into()),
        messages,
        tools: vec![ToolSpec {
            name: "search".into(),
            description: "Searches".into(),
            input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"], "additionalProperties": false}),
            strict: true,
        }],
        output_schema: None,
    }
}

fn client(url: &str, provider: Catalogued, key: Option<&str>) -> OpenAi {
    OpenAi::new(provider, key.map(String::from), Some(url)).with_retry_delay(Duration::from_millis(5))
}

#[test]
fn a_conversation_with_tools_is_translated_both_ways() {
    let (url, received) = serve(vec![(
        200,
        answer(
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_2", "type": "function", "function": {"name": "search", "arguments": "{\"q\":\"rust\"}"}}]}),
            "tool_calls",
        ),
    )]);
    let mut model = ModelConfig::new("openai", "gpt-5");
    model.temperature = Some(0.2);
    model.effort = Some("low".into());
    let history = vec![
        json!({"role": "user", "content": "Find it"}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "Looking."}, {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "grenat"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "nothing", "is_error": true}]}),
    ];
    let response = client(&url, chat(), Some("sk-test")).complete(&request(&model, history)).unwrap();
    assert_eq!(response.stop_reason, "tool_use");
    let uses = response.tool_uses();
    assert_eq!(
        (uses[0].id.as_str(), uses[0].name.as_str(), &uses[0].input),
        ("call_2", "search", &json!({"q": "rust"}))
    );
    assert_eq!(
        (response.usage.input_tokens, response.usage.cache_read_input_tokens, response.usage.output_tokens),
        (100, 20, 7)
    );
    assert_eq!(response.model, "gpt-5-2026-08-01");

    let received = received.lock().unwrap();
    let r = &received[0];
    assert_eq!(r.path, "/v1/chat/completions");
    assert_eq!(r.headers["authorization"], "Bearer sk-test");
    let body = &r.body;
    assert_eq!(body["model"], "gpt-5");
    assert_eq!(body["max_completion_tokens"], 16_000);
    assert_eq!((body["temperature"].clone(), body["reasoning_effort"].clone()), (json!(0.2), json!("low")));
    assert_eq!(
        body["tools"][0],
        json!({"type": "function", "function": {"name": "search", "description": "Searches", "parameters": request(&model, vec![]).tools[0].input_schema, "strict": true}})
    );
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0], json!({"role": "system", "content": "be brief"}));
    assert_eq!(messages[1], json!({"role": "user", "content": "Find it"}));
    assert_eq!(
        messages[2],
        json!({"role": "assistant", "content": "Looking.", "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "search", "arguments": "{\"q\":\"grenat\"}"}}]})
    );
    assert_eq!(messages[3], json!({"role": "tool", "tool_call_id": "call_1", "content": "Error: nothing"}));
}

#[test]
fn structured_output_strict_or_in_json_mode() {
    let schema = json!({"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"], "additionalProperties": false});
    let model = ModelConfig::new("openai", "gpt-5");
    let mut strict = request(&model, vec![json!({"role": "user", "content": "Summarize"})]);
    strict.output_schema = Some(schema.clone());
    let body = chat_body(&strict, &chat()).unwrap();
    assert_eq!(
        body["response_format"],
        json!({"type": "json_schema", "json_schema": {"name": "answer", "schema": schema, "strict": true}})
    );
    // a provider without strict schemas: JSON mode, the schema in the instructions, tools not strict
    let body = chat_body(&strict, &provider("groq")).unwrap();
    assert_eq!(body["response_format"], json!({"type": "json_object"}));
    assert!(body["messages"][0]["content"].as_str().unwrap().contains("follows this JSON Schema"));
    assert!(body["tools"][0]["function"].get("strict").is_none());
    assert_eq!(body["max_tokens"], 16_000);
    assert!(body.get("max_completion_tokens").is_none());
}

#[test]
fn images_and_documents() {
    let model = ModelConfig::new("openai", "gpt-5");
    let content = json!([
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAA"}},
        {"type": "image", "source": {"type": "url", "url": "https://x/y.png"}},
        {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBE"}},
        {"type": "text", "text": "Read"},
    ]);
    let body = chat_body(&request(&model, vec![json!({"role": "user", "content": content})]), &chat()).unwrap();
    let parts = &body["messages"][1]["content"];
    assert_eq!(parts[0], json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}}));
    assert_eq!(parts[1], json!({"type": "image_url", "image_url": {"url": "https://x/y.png"}}));
    assert_eq!(
        parts[2],
        json!({"type": "file", "file": {"filename": "document.pdf", "file_data": "data:application/pdf;base64,JVBE"}})
    );
    assert_eq!(parts[3], json!({"type": "text", "text": "Read"}));
    let e = chat_body(&request(&model, vec![json!({"role": "user", "content": content})]), &provider("ollama"))
        .unwrap_err();
    assert_eq!(e.message, "the provider `ollama` does not read PDF documents");
}

#[test]
fn text_answers_refusals_and_truncation() {
    let (url, _) = serve(vec![
        (200, answer(json!({"role": "assistant", "content": "Hi"}), "stop")),
        (200, answer(json!({"role": "assistant", "content": null, "refusal": "I can't"}), "stop")),
        (200, answer(json!({"role": "assistant", "content": "cut"}), "length")),
    ]);
    let model = ModelConfig::new("openai", "gpt-5");
    let c = client(&url, chat(), Some("k"));
    let hi = c.complete(&request(&model, vec![json!({"role": "user", "content": "Hi"})])).unwrap();
    assert_eq!((hi.text(), hi.stop_reason.as_str()), ("Hi".to_string(), "end_turn"));
    assert_eq!(c.complete(&request(&model, vec![])).unwrap().stop_reason, "refusal");
    assert_eq!(c.complete(&request(&model, vec![])).unwrap().stop_reason, "max_tokens");
}

#[test]
fn retries_rate_limits_not_client_errors() {
    let (url, received) = serve(vec![
        (429, json!({"error": {"message": "slow down"}})),
        (503, json!([{"error": {"code": 503, "message": "overloaded"}}])),
        (200, answer(json!({"role": "assistant", "content": "ok"}), "stop")),
        (400, json!({"error": {"message": "invalid schema"}})),
    ]);
    let model = ModelConfig::new("openai", "gpt-5");
    let c = client(&url, chat(), Some("k"));
    assert_eq!(c.complete(&request(&model, vec![])).unwrap().text(), "ok");
    assert_eq!(c.complete(&request(&model, vec![])).unwrap_err().message, "HTTP 400: invalid schema");
    assert_eq!(received.lock().unwrap().len(), 4);
}

#[test]
fn a_local_server_needs_no_key() {
    let (url, received) = serve(vec![(200, answer(json!({"role": "assistant", "content": "local"}), "stop"))]);
    let model = ModelConfig::new("ollama", "llama3.3");
    assert_eq!(client(&url, provider("ollama"), None).complete(&request(&model, vec![])).unwrap().text(), "local");
    assert!(!received.lock().unwrap()[0].headers.contains_key("authorization"));
}

#[test]
fn every_catalogued_provider_is_reachable() {
    for p in catalog::PROVIDERS {
        assert!(p.base_url.starts_with("https://") || p.name == "ollama", "{}", p.name);
        assert!(p.key_variable.ends_with("_API_KEY"), "{}", p.name);
    }
    assert!(catalog::provider("nope").is_none());
    assert!(catalog::names().starts_with("anthropic, openai, gemini"));
}

// ── OpenAI's Responses API ─────────────────────────────────────

#[test]
fn openai_itself_is_spoken_to_through_the_responses_api() {
    let output = json!({
        "model": "gpt-5.4-mini-2026-03-17",
        "status": "completed",
        "output": [
            {"type": "reasoning", "id": "rs_1", "summary": []},
            {"type": "function_call", "id": "fc_1", "call_id": "call_9", "name": "search", "arguments": "{\"q\":\"rust\"}"}
        ],
        "usage": {"input_tokens": 108, "output_tokens": 33, "input_tokens_details": {"cached_tokens": 8}}
    });
    let (url, received) = serve(vec![(200, output)]);
    let mut model = ModelConfig::new("openai", "gpt-5.4-mini");
    model.effort = Some("low".into());
    let history = vec![
        json!({"role": "user", "content": "Find it"}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "Looking."}, {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "grenat"}}]}),
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "nothing"}]}),
    ];
    let response = client(&url, provider("openai"), Some("sk-test")).complete(&request(&model, history)).unwrap();
    assert_eq!(response.stop_reason, "tool_use");
    let uses = response.tool_uses();
    assert_eq!((uses[0].id.as_str(), &uses[0].input), ("call_9", &json!({"q": "rust"})));
    assert_eq!((response.usage.input_tokens, response.usage.cache_read_input_tokens), (100, 8));

    let received = received.lock().unwrap();
    assert_eq!(received[0].path, "/v1/responses");
    let body = &received[0].body;
    assert_eq!((body["store"].clone(), body["max_output_tokens"].clone()), (json!(false), json!(16_000)));
    assert_eq!(body["instructions"], "be brief");
    assert_eq!(body["reasoning"], json!({"effort": "low"}));
    assert_eq!(body["tools"][0]["name"], "search");
    assert_eq!(body["tools"][0]["strict"], true);
    assert_eq!(
        body["input"],
        json!([
            {"role": "user", "content": "Find it"},
            {"role": "assistant", "content": "Looking."},
            {"type": "function_call", "call_id": "call_1", "name": "search", "arguments": "{\"q\":\"grenat\"}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "nothing"}
        ])
    );
}

#[test]
fn responses_structured_output_media_refusals_and_truncation() {
    let schema = json!({"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"], "additionalProperties": false});
    let model = ModelConfig::new("openai", "gpt-5.4-mini");
    let content = json!([
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAA"}},
        {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBE"}},
        {"type": "text", "text": "Read"},
    ]);
    let mut asked = request(&model, vec![json!({"role": "user", "content": content})]);
    asked.output_schema = Some(schema.clone());
    let body = grenat_llm::responses_body(&asked, &provider("openai")).unwrap();
    assert_eq!(
        body["text"]["format"],
        json!({"type": "json_schema", "name": "answer", "schema": schema, "strict": true})
    );
    assert_eq!(
        body["input"][0]["content"],
        json!([
            {"type": "input_image", "image_url": "data:image/png;base64,AAA"},
            {"type": "input_file", "filename": "document.pdf", "file_data": "data:application/pdf;base64,JVBE"},
            {"type": "input_text", "text": "Read"}
        ])
    );
    let message = |content: Json| json!({"model": "m", "status": "completed", "output": [{"type": "message", "role": "assistant", "content": content}], "usage": {}});
    let (url, _) = serve(vec![
        (200, message(json!([{"type": "output_text", "text": "{\"title\":\"x\"}"}]))),
        (200, message(json!([{"type": "refusal", "refusal": "no"}]))),
        (
            200,
            json!({"model": "m", "status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output": [], "usage": {}}),
        ),
    ]);
    let c = client(&url, provider("openai"), Some("k"));
    let answer = c.complete(&asked).unwrap();
    assert_eq!((answer.text(), answer.stop_reason.as_str()), ("{\"title\":\"x\"}".to_string(), "end_turn"));
    assert_eq!(c.complete(&asked).unwrap().stop_reason, "refusal");
    assert_eq!(c.complete(&asked).unwrap().stop_reason, "max_tokens");
}
