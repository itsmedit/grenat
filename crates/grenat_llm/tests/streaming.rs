//! Streamed answers, read from local servers that play each wire format:
//! the Messages API, Chat Completions and the Responses API. The servers
//! send their events in small pieces, a pause between each — lines and
//! multi-byte characters split between two reads — as a network may.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grenat_llm::catalog::{self, Catalogued};
use grenat_llm::{
    Anthropic, Cassette, Delta, ModelConfig, OpenAi, Provider, Request, Response, STOPPED, Scripted, ToolSpec,
};
use serde_json::{Value as Json, json};

/// A request the server received: its path, headers and JSON body.
struct Received {
    path: String,
    headers: HashMap<String, String>,
    body: Json,
}

/// A reply: a status, and a body sent in these pieces.
struct Reply {
    status: u16,
    pieces: Vec<Vec<u8>>,
}

/// An event stream, cut every `size` bytes.
fn stream(text: &str, size: usize) -> Reply {
    Reply { status: 200, pieces: text.as_bytes().chunks(size).map(<[u8]>::to_vec).collect() }
}

fn failure(status: u16, body: Json) -> Reply {
    Reply { status, pieces: vec![body.to_string().into_bytes()] }
}

/// Answers each connection with the next reply, its body closed by the
/// end of the connection.
fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Received>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();
    std::thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
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
                body: serde_json::from_slice(&raw).unwrap(),
            });
            let kind = if reply.status == 200 { "text/event-stream" } else { "application/json" };
            let head = format!("HTTP/1.1 {} X\r\ncontent-type: {kind}\r\nconnection: close\r\n\r\n", reply.status);
            stream.write_all(head.as_bytes()).unwrap();
            for piece in reply.pieces {
                // the client may have stopped reading
                if stream.write_all(&piece).and_then(|()| stream.flush()).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    });
    (url, received)
}

/// What a sink received: text, and tool inputs as `name(json)`.
#[derive(Default, Debug, PartialEq)]
struct Pieces {
    text: Vec<String>,
    tools: Vec<String>,
}

/// Streams `request` through `provider`, collecting the pieces.
fn collect(provider: &dyn Provider, request: &Request) -> (Result<Response, String>, Pieces) {
    let mut pieces = Pieces::default();
    let result = provider.stream(request, &mut |delta| {
        match delta {
            Delta::Text(text) => pieces.text.push(text.to_string()),
            Delta::ToolInput { id, name, json } => pieces.tools.push(format!("{id}:{name}:{json}")),
        }
        ControlFlow::Continue(())
    });
    (result.map_err(|e| e.message), pieces)
}

fn ask(model: &ModelConfig) -> Request<'_> {
    Request {
        model,
        system: Some("be brief".into()),
        messages: vec![json!({"role": "user", "content": "Hello"})],
        tools: vec![ToolSpec {
            name: "search".into(),
            description: "Searches".into(),
            input_schema: json!({"type": "object"}),
            strict: true,
        }],
        output_schema: None,
    }
}

/// `event: <name>` lines and their data, as a server writes them.
fn sse(events: &[(&str, Json)]) -> String {
    events.iter().map(|(name, data)| format!("event: {name}\ndata: {data}\n\n")).collect()
}

/// Chat Completions chunks: data only, `[DONE]` last.
fn chunks(chunks: &[Json], done: bool) -> String {
    let mut text: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    if done {
        text.push_str("data: [DONE]\n\n");
    }
    text
}

// ── Messages API ──────────────────────────────────────────────

fn anthropic(url: &str) -> Anthropic {
    Anthropic::new("sk-test", url).with_retry_delay(Duration::from_millis(5))
}

fn message_start(input: u64) -> (&'static str, Json) {
    (
        "message_start",
        json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-5", "stop_reason": null, "usage": {"input_tokens": input, "cache_read_input_tokens": 30, "output_tokens": 1}}}),
    )
}

fn text_block(index: u64) -> (&'static str, Json) {
    (
        "content_block_start",
        json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}),
    )
}

fn text_delta(index: u64, text: &str) -> (&'static str, Json) {
    (
        "content_block_delta",
        json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": text}}),
    )
}

fn block_stop(index: u64) -> (&'static str, Json) {
    ("content_block_stop", json!({"type": "content_block_stop", "index": index}))
}

fn message_end(reason: &str, output: u64) -> Vec<(&'static str, Json)> {
    vec![
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": reason, "stop_sequence": null}, "usage": {"output_tokens": output}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]
}

#[test]
fn a_messages_stream_gives_its_text_as_it_comes_and_the_whole_response() {
    let mut events = vec![message_start(12), text_block(0), ("ping", json!({"type": "ping"}))];
    events.extend([text_delta(0, "Bonjour, "), text_delta(0, "日本"), text_delta(0, " ✓"), block_stop(0)]);
    events.extend(message_end("end_turn", 9));
    // three bytes a write: "日本" and "✓" arrive split between reads
    let (url, received) = serve(vec![stream(&sse(&events), 3)]);
    let model = ModelConfig::new("anthropic", "claude-opus-5");
    let (response, pieces) = collect(&anthropic(&url), &ask(&model));
    let response = response.unwrap();
    assert_eq!(pieces.text, ["Bonjour, ", "日本", " ✓"]);
    assert_eq!(response.text(), "Bonjour, 日本 ✓");
    assert_eq!(response.stop_reason, "end_turn");
    assert_eq!(response.model, "claude-opus-5");
    // the input from `message_start`, the output from the last `message_delta`
    assert_eq!(
        (response.usage.input_tokens, response.usage.cache_read_input_tokens, response.usage.output_tokens),
        (12, 30, 9)
    );
    let received = received.lock().unwrap();
    assert_eq!(received[0].path, "/v1/messages");
    assert_eq!(received[0].headers["x-api-key"], "sk-test");
    assert_eq!(received[0].body["stream"], true);
    assert_eq!(received[0].body["system"][0]["text"], "be brief");
}

#[test]
fn a_messages_stream_with_tool_calls_gives_their_input_in_pieces() {
    let mut events = vec![message_start(40), text_block(0), text_delta(0, "Looking."), block_stop(0)];
    events.push((
        "content_block_start",
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {}}}),
    ));
    for piece in ["", "{\"q\": \"gre", "nat é", "t\"}"] {
        events.push((
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": piece}}),
        ));
    }
    events.push(block_stop(1));
    events.extend(message_end("tool_use", 20));
    let (url, _) = serve(vec![stream(&sse(&events), 5)]);
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let (response, pieces) = collect(&anthropic(&url), &ask(&model));
    let response = response.unwrap();
    // an empty piece is not given
    assert_eq!(
        pieces,
        Pieces {
            text: vec!["Looking.".into()],
            tools: vec![
                "toolu_1:search:{\"q\": \"gre".into(),
                "toolu_1:search:nat é".into(),
                "toolu_1:search:t\"}".into()
            ]
        }
    );
    assert_eq!(response.stop_reason, "tool_use");
    let calls = response.tool_uses();
    assert_eq!((calls[0].id.as_str(), &calls[0].input), ("toolu_1", &json!({"q": "grenat ét"})));
    // sent back unchanged in the history, as a call without streaming would be
    assert_eq!(
        response.content[1],
        json!({"type": "tool_use", "id": "toolu_1", "name": "search", "input": {"q": "grenat ét"}})
    );
}

#[test]
fn an_error_before_any_text_is_retried_one_after_is_not() {
    let overloaded =
        ("error", json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}));
    let mut whole = vec![message_start(5), text_block(0), text_delta(0, "ok"), block_stop(0)];
    whole.extend(message_end("end_turn", 2));
    let (url, received) = serve(vec![
        failure(529, json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})),
        stream(&sse(&[message_start(5), overloaded.clone()]), 64),
        stream(&sse(&whole), 64),
    ]);
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let (response, pieces) = collect(&anthropic(&url), &ask(&model));
    assert_eq!(response.unwrap().text(), "ok");
    assert_eq!(pieces.text, ["ok"]);
    assert_eq!(received.lock().unwrap().len(), 3);

    // part of the answer was given: no second attempt, the error is the call's
    let (url, received) =
        serve(vec![stream(&sse(&[message_start(5), text_block(0), text_delta(0, "Hal"), overloaded]), 64)]);
    let (response, pieces) = collect(&anthropic(&url), &ask(&model));
    assert_eq!(response.unwrap_err(), "Overloaded (overloaded_error, while streaming)");
    assert_eq!(pieces.text, ["Hal"]);
    assert_eq!(received.lock().unwrap().len(), 1);

    // a client error is never retried
    let (url, received) = serve(vec![failure(
        400,
        json!({"type": "error", "error": {"type": "invalid_request_error", "message": "bad model"}}),
    )]);
    assert_eq!(collect(&anthropic(&url), &ask(&model)).0.unwrap_err(), "HTTP 400: bad model");
    assert_eq!(received.lock().unwrap().len(), 1);
}

#[test]
fn a_stream_cut_short_fails_and_a_consumer_may_stop_it() {
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let cut = sse(&[message_start(5), text_block(0), text_delta(0, "Hal")]);
    let (url, _) = serve(vec![stream(&cut, 7)]);
    let (response, _) = collect(&anthropic(&url), &ask(&model));
    assert_eq!(response.unwrap_err(), "the stream ended before the answer was complete");

    let mut events = vec![message_start(5), text_block(0), text_delta(0, "one"), text_delta(0, "two")];
    events.extend(message_end("end_turn", 2));
    let (url, _) = serve(vec![stream(&sse(&events), 16)]);
    let mut seen = Vec::new();
    let result = anthropic(&url).stream(&ask(&model), &mut |delta| {
        if let Delta::Text(text) = delta {
            seen.push(text.to_string());
        }
        ControlFlow::Break(())
    });
    let stopped = result.unwrap_err();
    assert_eq!(stopped.message, STOPPED);
    assert_eq!(seen, ["one"]);
    // billed all the same: the usage the stream said before it stopped
    let billed = stopped.billed.expect("the usage so far");
    assert_eq!(
        (billed.model.as_str(), billed.usage.input_tokens, billed.usage.cache_read_input_tokens),
        ("claude-opus-5", 5, 30)
    );
}

#[test]
fn after_a_decline_mid_answer_the_fallback_model_goes_on() {
    let mut events = vec![message_start(5), text_block(0), text_delta(0, "Once upon"), block_stop(0)];
    events.push((
        "content_block_start",
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "fallback", "from": {"model": "claude-opus-5"}, "to": {"model": "claude-opus-4-8"}}}),
    ));
    events.extend([block_stop(1), text_block(2), text_delta(2, " a time."), block_stop(2)]);
    // the last usage says what each attempt cost
    let iterations = json!([
        {"type": "message", "model": "claude-opus-5", "input_tokens": 5, "output_tokens": 2000, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0},
        {"type": "fallback_message", "model": "claude-opus-4-8", "input_tokens": 9, "output_tokens": 12, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}
    ]);
    events.push((
        "message_delta",
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 12, "iterations": iterations}}),
    ));
    events.push(("message_stop", json!({"type": "message_stop"})));
    let (url, _) = serve(vec![stream(&sse(&events), 32)]);
    let model = ModelConfig::new("anthropic", "claude-opus-5");
    let (response, pieces) = collect(&anthropic(&url), &ask(&model));
    let response = response.unwrap();
    assert_eq!(pieces.text, ["Once upon", " a time."]);
    assert_eq!(response.text(), "Once upon a time.");
    // the model that finished the answer is billed
    assert_eq!(response.model, "claude-opus-4-8");
    assert_eq!(response.content[1]["type"], "fallback");
    // and the attempt that declined after writing, at its own model
    assert_eq!(response.declined.len(), 1);
    assert_eq!(response.declined[0].model, "claude-opus-5");
    assert_eq!((response.declined[0].usage.input_tokens, response.declined[0].usage.output_tokens), (5, 2000));
}

// ── Chat Completions ──────────────────────────────────────────

fn provider(name: &str) -> Catalogued {
    *catalog::provider(name).unwrap()
}

fn openai(url: &str, provider: Catalogued) -> OpenAi {
    OpenAi::new(provider, Some("sk-test".into()), Some(url)).with_retry_delay(Duration::from_millis(5))
}

fn chunk(delta: Json, finish: Option<&str>) -> Json {
    json!({"id": "c1", "object": "chat.completion.chunk", "model": "deepseek-chat", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}], "usage": null})
}

#[test]
fn chat_completions_chunks_give_text_then_the_usage() {
    let body = chunks(
        &[
            chunk(json!({"role": "assistant", "content": ""}), None),
            chunk(json!({"content": "Salut "}), None),
            chunk(json!({"content": "à toi"}), None),
            chunk(json!({}), Some("stop")),
            json!({"id": "c1", "model": "deepseek-chat", "choices": [], "usage": {"prompt_tokens": 50, "completion_tokens": 4, "prompt_cache_hit_tokens": 10}}),
        ],
        true,
    );
    let (url, received) = serve(vec![stream(&body, 4)]);
    let model = ModelConfig::new("deepseek", "deepseek-chat");
    let (response, pieces) = collect(&openai(&url, provider("deepseek")), &ask(&model));
    let response = response.unwrap();
    assert_eq!(pieces.text, ["Salut ", "à toi"]);
    assert_eq!(response.text(), "Salut à toi");
    assert_eq!(response.stop_reason, "end_turn");
    assert_eq!(
        (response.usage.input_tokens, response.usage.cache_read_input_tokens, response.usage.output_tokens),
        (40, 10, 4)
    );
    let received = received.lock().unwrap();
    assert_eq!(received[0].path, "/chat/completions");
    assert_eq!(received[0].body["stream"], true);
    assert_eq!(received[0].body["stream_options"], json!({"include_usage": true}));
}

#[test]
fn chat_completions_tool_calls_arrive_in_pieces_by_index() {
    let call = |index: u64, id: Option<&str>, name: Option<&str>, arguments: &str| {
        let mut function = json!({"arguments": arguments});
        if let Some(name) = name {
            function["name"] = json!(name);
        }
        let mut call = json!({"index": index, "type": "function", "function": function});
        if let Some(id) = id {
            call["id"] = json!(id);
        }
        chunk(json!({"tool_calls": [call]}), None)
    };
    let body = chunks(
        &[
            call(0, Some("call_a"), Some("search"), ""),
            call(0, None, None, "{\"q\":"),
            call(1, Some("call_b"), Some("search"), "{\"q\": \"b\"}"),
            call(0, None, None, " \"a\"}"),
            // Groq's usage, unasked
            json!({"model": "llama", "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}], "x_groq": {"usage": {"prompt_tokens": 9, "completion_tokens": 3}}}),
        ],
        true,
    );
    let (url, received) = serve(vec![stream(&body, 9)]);
    let model = ModelConfig::new("groq", "llama");
    let (response, pieces) = collect(&openai(&url, provider("groq")), &ask(&model));
    let response = response.unwrap();
    assert_eq!(pieces.tools, ["call_a:search:{\"q\":", "call_b:search:{\"q\": \"b\"}", "call_a:search: \"a\"}"]);
    assert_eq!(response.stop_reason, "tool_use");
    let calls = response.tool_uses();
    assert_eq!(calls.len(), 2);
    assert_eq!((calls[0].id.as_str(), &calls[0].input), ("call_a", &json!({"q": "a"})));
    assert_eq!((calls[1].id.as_str(), &calls[1].input), ("call_b", &json!({"q": "b"})));
    assert_eq!((response.usage.input_tokens, response.usage.output_tokens), (9, 3));
    assert_eq!(response.model, "llama");
    assert_eq!(received.lock().unwrap()[0].body["stream_options"], json!({"include_usage": true}));
}

#[test]
fn chat_completions_errors_mid_stream_and_servers_that_skip_done() {
    // Mistral documents no `stream_options`: none is sent; its stream closes after the last chunk
    let body = chunks(&[chunk(json!({"content": "fin"}), Some("length"))], false);
    let (url, received) = serve(vec![stream(&body, 64)]);
    let model = ModelConfig::new("mistral", "mistral-large");
    let (response, _) = collect(&openai(&url, provider("mistral")), &ask(&model));
    assert_eq!(response.unwrap().stop_reason, "max_tokens");
    assert!(received.lock().unwrap()[0].body.get("stream_options").is_none());

    let body = chunks(
        &[chunk(json!({"content": "par"}), None), json!({"error": {"message": "upstream timeout", "code": 504}})],
        false,
    );
    let (url, received) = serve(vec![stream(&body, 64)]);
    let (response, pieces) = collect(&openai(&url, provider("mistral")), &ask(&model));
    assert_eq!(response.unwrap_err(), "upstream timeout (while streaming)");
    assert_eq!(pieces.text, ["par"]);
    assert_eq!(received.lock().unwrap().len(), 1);

    let (url, _) = serve(vec![stream(&chunks(&[chunk(json!({"content": "par"}), None)], false), 64)]);
    let (response, _) = collect(&openai(&url, provider("mistral")), &ask(&model));
    assert_eq!(response.unwrap_err(), "the stream ended before the answer was complete");
}

// ── Responses API ─────────────────────────────────────────────

fn typed(events: &[Json]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())).collect()
}

fn completed(output: Json, status: &str) -> Json {
    json!({"type": format!("response.{status}"), "sequence_number": 9, "response": {
        "id": "resp_1", "status": status, "model": "gpt-5.6-2026-09-01", "output": output,
        "incomplete_details": if status == "incomplete" { json!({"reason": "max_output_tokens"}) } else { Json::Null },
        "usage": {"input_tokens": 100, "output_tokens": 8, "input_tokens_details": {"cached_tokens": 60}}
    }})
}

#[test]
fn a_responses_stream_gives_text_and_function_calls() {
    let message = json!({"id": "msg_1", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Je cherche."}]});
    let call = json!({"id": "fc_1", "type": "function_call", "call_id": "call_9", "name": "search", "arguments": "{\"q\":\"x\"}"});
    let body = typed(&[
        json!({"type": "response.created", "sequence_number": 0, "response": {"id": "resp_1", "status": "in_progress", "output": []}}),
        json!({"type": "response.output_item.added", "sequence_number": 1, "output_index": 0, "item": {"id": "msg_1", "type": "message", "content": []}}),
        json!({"type": "response.output_text.delta", "sequence_number": 2, "item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": "Je ", "logprobs": []}),
        json!({"type": "response.output_text.delta", "sequence_number": 3, "item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": "cherche.", "logprobs": []}),
        json!({"type": "response.output_item.added", "sequence_number": 4, "output_index": 1, "item": {"id": "fc_1", "type": "function_call", "call_id": "call_9", "name": "search", "arguments": ""}}),
        json!({"type": "response.function_call_arguments.delta", "sequence_number": 5, "item_id": "fc_1", "output_index": 1, "delta": "{\"q\":"}),
        json!({"type": "response.function_call_arguments.delta", "sequence_number": 6, "item_id": "fc_1", "output_index": 1, "delta": "\"x\"}"}),
        json!({"type": "response.function_call_arguments.done", "sequence_number": 7, "item_id": "fc_1", "output_index": 1, "arguments": "{\"q\":\"x\"}"}),
        completed(json!([message, call]), "completed"),
    ]);
    let (url, received) = serve(vec![stream(&body, 6)]);
    let model = ModelConfig::new("openai", "gpt-5.6");
    let (response, pieces) = collect(&openai(&url, provider("openai")), &ask(&model));
    let response = response.unwrap();
    assert_eq!(pieces.text, ["Je ", "cherche."]);
    assert_eq!(pieces.tools, ["call_9:search:{\"q\":", "call_9:search:\"x\"}"]);
    assert_eq!(response.text(), "Je cherche.");
    assert_eq!(response.stop_reason, "tool_use");
    assert_eq!(response.tool_uses()[0].input, json!({"q": "x"}));
    assert_eq!((response.usage.input_tokens, response.usage.cache_read_input_tokens), (40, 60));
    let received = received.lock().unwrap();
    assert_eq!(received[0].path, "/responses");
    assert_eq!(received[0].body["stream"], true);
    assert!(received[0].body.get("stream_options").is_none());
}

#[test]
fn a_responses_stream_cut_short_failed_or_in_error() {
    let model = ModelConfig::new("openai", "gpt-5.6");
    let text = |delta: &str| json!({"type": "response.output_text.delta", "sequence_number": 1, "item_id": "m", "output_index": 0, "content_index": 0, "delta": delta, "logprobs": []});
    // the output the last event leaves out comes from the items done
    let item = json!({"id": "m", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "trop"}]});
    let body = typed(&[
        text("trop"),
        json!({"type": "response.output_item.done", "sequence_number": 2, "output_index": 0, "item": item}),
        completed(json!([]), "incomplete"),
    ]);
    let (url, _) = serve(vec![stream(&body, 64)]);
    let response = collect(&openai(&url, provider("openai")), &ask(&model)).0.unwrap();
    assert_eq!((response.text().as_str(), response.stop_reason.as_str()), ("trop", "max_tokens"));

    let failed = json!({"type": "response.failed", "sequence_number": 3, "response": {"status": "failed", "error": {"code": "invalid_prompt", "message": "The prompt was flagged."}, "output": []}});
    let (url, received) = serve(vec![stream(&typed(&[failed]), 64)]);
    assert_eq!(
        collect(&openai(&url, provider("openai")), &ask(&model)).0.unwrap_err(),
        "The prompt was flagged. (while streaming)"
    );
    assert_eq!(received.lock().unwrap().len(), 1);

    // a server error before any text: tried again
    let error =
        json!({"type": "error", "sequence_number": 1, "code": "server_error", "message": "try again", "param": null});
    let whole = typed(&[
        text("ok"),
        completed(json!([{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}]), "completed"),
    ]);
    let (url, received) = serve(vec![stream(&typed(&[error]), 64), stream(&whole, 64)]);
    let (response, pieces) = collect(&openai(&url, provider("openai")), &ask(&model));
    assert_eq!(response.unwrap().text(), "ok");
    assert_eq!(pieces.text, ["ok"]);
    assert_eq!(received.lock().unwrap().len(), 2);
}

// ── Providers that do not stream ──────────────────────────────

#[test]
fn a_provider_that_does_not_stream_gives_its_answer_as_one_piece() {
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let scripted = Scripted::new([
        Response::text_reply("whole answer"),
        Response::tool_call("t1", "final_answer", json!({"value": "done"})),
    ]);
    let (response, pieces) = collect(&scripted, &ask(&model));
    assert_eq!(response.unwrap().text(), "whole answer");
    assert_eq!(pieces.text, ["whole answer"]);
    let (_, pieces) = collect(&scripted, &ask(&model));
    assert_eq!(pieces.tools, ["t1:final_answer:{\"value\":\"done\"}"]);
}

#[test]
fn a_cassette_records_a_streamed_call_and_replays_it_whole() {
    let mut events = vec![message_start(5), text_block(0), text_delta(0, "rec"), text_delta(0, "orded")];
    events.push(block_stop(0));
    events.extend(message_end("end_turn", 2));
    let (url, _) = serve(vec![stream(&sse(&events), 8)]);
    let dir = std::env::temp_dir().join(format!("grenat-stream-cassette-{}", std::process::id()));
    let path = dir.join("calls.json");
    let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
    let recorder = Cassette::record(&path, Arc::new(anthropic(&url)));
    let (response, pieces) = collect(&recorder, &ask(&model));
    assert_eq!((response.unwrap().text(), pieces.text.len()), ("recorded".to_string(), 2));
    recorder.save().unwrap();
    let player = Cassette::replay(&path).unwrap();
    let (response, pieces) = collect(&player, &ask(&model));
    assert_eq!(response.unwrap().text(), "recorded");
    assert_eq!(pieces.text, ["recorded"]);
    std::fs::remove_dir_all(dir).unwrap();
}
