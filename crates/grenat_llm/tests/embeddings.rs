//! Embeddings through each provider that makes them — OpenAI, Gemini (its
//! compatible endpoint), Mistral, Ollama and Voyage — tested against a local
//! server that plays their `POST /embeddings`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grenat_llm::catalog::{self, Catalogued};
use grenat_llm::{EmbeddingRequest, ModelConfig, ModelKind, OpenAi, Provider, Request};
use serde_json::{Value as Json, json};

struct Received {
    path: String,
    headers: HashMap<String, String>,
    body: Json,
}

/// Starts a server answering each request with `reply(body)`: (status, body).
fn serve(
    requests: usize,
    reply: impl Fn(&Json) -> (u16, Json) + Send + 'static,
) -> (String, Arc<Mutex<Vec<Received>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();
    std::thread::spawn(move || {
        for _ in 0..requests {
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
            let body: Json = serde_json::from_slice(&raw).unwrap_or(Json::Null);
            let (status, answer) = reply(&body);
            log.lock().unwrap().push(Received {
                path: request_line.split_whitespace().nth(1).unwrap().to_string(),
                headers,
                body,
            });
            let payload = answer.to_string();
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

/// An answer in OpenAI's format: a vector `[i, len]` for the i-th text,
/// listed in reverse (the index orders them), with `usage` as `field`.
fn vectors(body: &Json, usage_field: &str) -> (u16, Json) {
    let inputs = body["input"].as_array().unwrap();
    let data: Vec<Json> = (0..inputs.len())
        .rev()
        .map(|i| json!({"object": "embedding", "index": i, "embedding": [i as f64, inputs[i].as_str().unwrap().len() as f64]}))
        .collect();
    (200, json!({"object": "list", "model": body["model"], "data": data, "usage": {usage_field: 3 * inputs.len()}}))
}

fn provider(name: &str) -> Catalogued {
    *catalog::provider(name).unwrap()
}

fn model(provider: &str, name: &str, dimensions: Option<u32>) -> ModelConfig {
    let mut model = ModelConfig::new(provider, name);
    model.kind = ModelKind::Embedding;
    model.dimensions = dimensions;
    model
}

fn client(url: &str, provider: Catalogued, key: Option<&str>) -> OpenAi {
    OpenAi::new(provider, key.map(String::from), Some(url)).with_retry_delay(Duration::from_millis(5))
}

fn texts(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|t| t.to_string()).collect()
}

#[test]
fn each_provider_s_request_and_answer() {
    // (provider, model, the field of the size, the field of the tokens, a key)
    let cases = [
        ("openai", "text-embedding-3-small", "dimensions", "prompt_tokens", Some("sk-openai")),
        ("gemini", "gemini-embedding-001", "dimensions", "prompt_tokens", Some("gm-key")),
        ("mistral", "mistral-embed", "output_dimension", "prompt_tokens", Some("ms-key")),
        ("ollama", "nomic-embed-text", "dimensions", "prompt_tokens", None),
        ("voyage", "voyage-3.5", "output_dimension", "total_tokens", Some("pa-key")),
    ];
    for (name, model_name, size_field, usage_field, key) in cases {
        let (url, received) = serve(1, move |body| vectors(body, usage_field));
        let model = model(name, model_name, Some(256));
        let request = EmbeddingRequest { model: &model, inputs: texts(&["refund", "invoices"]) };
        let answer = client(&url, provider(name), key).embed(&request).unwrap();
        assert_eq!(answer.vectors, [[0.0, 6.0], [1.0, 8.0]], "{name}");
        assert_eq!((answer.usage.input_tokens, answer.usage.output_tokens), (6, 0), "{name}");
        assert_eq!(answer.model, model_name);
        let received = received.lock().unwrap();
        let r = &received[0];
        assert_eq!(r.path, "/v1/embeddings", "{name}");
        assert_eq!(r.body, json!({"model": model_name, "input": ["refund", "invoices"], size_field: 256}), "{name}");
        match key {
            Some(key) => assert_eq!(r.headers["authorization"], format!("Bearer {key}"), "{name}"),
            None => assert!(!r.headers.contains_key("authorization"), "{name}: a local server needs no key"),
        }
    }
}

#[test]
fn many_texts_go_in_several_requests_in_order() {
    let (url, received) = serve(2, |body| vectors(body, "prompt_tokens"));
    let model = model("openai", "text-embedding-3-small", None);
    let inputs: Vec<String> = (0..2050).map(|i| format!("text {i}")).collect();
    let request = EmbeddingRequest { model: &model, inputs };
    let answer = client(&url, provider("openai"), Some("k")).embed(&request).unwrap();
    assert_eq!(answer.vectors.len(), 2050);
    // the 2,049th text is the first of the second request
    assert_eq!(answer.vectors[2048], [0.0, 9.0]);
    assert_eq!(answer.usage.input_tokens, 3 * 2050);
    let received = received.lock().unwrap();
    let sizes: Vec<usize> = received.iter().map(|r| r.body["input"].as_array().unwrap().len()).collect();
    assert_eq!(sizes, [2048, 2]);
    assert!(received[0].body.get("dimensions").is_none(), "no size asked: the model's own");
}

#[test]
fn overload_is_retried_and_client_errors_are_said() {
    let calls = Arc::new(Mutex::new(0));
    let count = calls.clone();
    let (url, _) = serve(2, move |body| {
        let mut n = count.lock().unwrap();
        *n += 1;
        if *n == 1 { (429, json!({"error": {"message": "slow down"}})) } else { vectors(body, "prompt_tokens") }
    });
    let model = model("mistral", "mistral-embed", None);
    let request = EmbeddingRequest { model: &model, inputs: texts(&["a"]) };
    assert_eq!(client(&url, provider("mistral"), Some("k")).embed(&request).unwrap().vectors.len(), 1);

    let (url, _) = serve(1, |_| (400, json!({"detail": "bad model"})));
    let error = client(&url, provider("voyage"), Some("k")).embed(&request).unwrap_err();
    assert_eq!(error.message, "HTTP 400: bad model");

    // fewer vectors than texts: refused, not shifted
    let (url, _) = serve(1, |_| (200, json!({"data": [{"index": 0, "embedding": [1.0]}]})));
    let two = EmbeddingRequest { model: &model, inputs: texts(&["a", "b"]) };
    let error = client(&url, provider("gemini"), Some("k")).embed(&two).unwrap_err();
    assert!(error.message.contains("2 texts sent, 1 vectors received"), "{}", error.message);
}

#[test]
fn providers_refuse_what_they_cannot_do() {
    let unreachable = "http://127.0.0.1:1/v1";
    // Voyage makes no chat
    let chat = ModelConfig::new("voyage", "voyage-3.5");
    let request = Request { model: &chat, system: None, messages: vec![], tools: vec![], output_schema: None };
    let error = client(unreachable, provider("voyage"), Some("k")).complete(&request).unwrap_err();
    assert!(error.message.contains("only makes embeddings"), "{}", error.message);
    // Groq makes no embeddings here
    let groq = model("groq", "x", None);
    let request = EmbeddingRequest { model: &groq, inputs: texts(&["a"]) };
    let error = client(unreachable, provider("groq"), Some("k")).embed(&request).unwrap_err();
    assert!(error.message.contains("makes no embeddings in Grenat"), "{}", error.message);
    assert!(error.message.contains("openai, gemini, mistral, ollama, voyage"), "{}", error.message);
    let anthropic = catalog::refuses(&provider("anthropic"), ModelKind::Embedding).unwrap();
    assert!(anthropic.starts_with("Anthropic has no embeddings API: declare a `voyage` model"), "{anthropic}");
    assert_eq!(catalog::refuses(&provider("openai"), ModelKind::Embedding), None);
    assert_eq!(catalog::refuses(&provider("openai"), ModelKind::Chat), None);
    assert_eq!(catalog::default_dimensions("text-embedding-3-large"), Some(3072));
    assert_eq!(catalog::default_dimensions("voyage-3.5-lite"), Some(1024));
    assert_eq!(catalog::default_dimensions("gpt-5"), None);
}
