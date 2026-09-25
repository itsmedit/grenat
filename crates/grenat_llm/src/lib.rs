//! Access to language models.
//!
//! The runtime only depends on the [`Provider`] trait: the real implementation
//! ([`Anthropic`]) talks HTTP to the Messages API, while [`Scripted`] replays
//! prepared replies for tests, [`Mock`] shapes test answers into whatever
//! each request expects, and [`Cassette`] records real calls once to replay
//! them.

mod anthropic;
mod cassette;
mod mock;
mod pricing;
mod scripted;
mod types;
mod wire;

pub use anthropic::Anthropic;
pub use cassette::Cassette;
pub use mock::{Mock, MockReply};
pub use pricing::cost_usd;
pub use scripted::Scripted;
pub use types::*;
pub(crate) use wire::parse_response;
pub use wire::request_body;

#[cfg(test)]
mod tests {
    use crate::*;
    use serde_json::json;

    #[test]
    fn body_has_tools_structured_output_and_effort() {
        let mut model = ModelConfig::new("anthropic", "claude-opus-5");
        model.effort = Some("low".into());
        assert!(model.fallbacks);
        assert!(!ModelConfig::new("anthropic", "claude-haiku-4-5").fallbacks);
        let request = Request {
            model: &model,
            system: Some("sys".into()),
            messages: vec![json!({"role": "user", "content": "hi"})],
            tools: vec![ToolSpec {
                name: "t".into(),
                description: "d".into(),
                input_schema: json!({"type": "object"}),
                strict: true,
            }],
            output_schema: Some(json!({"type": "object"})),
        };
        let body = request_body(&request);
        assert_eq!(body["system"], "sys");
        assert_eq!(body["tools"][0]["strict"], true);
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["fallbacks"], "default");
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn costs() {
        let usage = Usage { input_tokens: 1_000_000, output_tokens: 1_000_000, ..Usage::default() };
        assert_eq!(cost_usd("claude-haiku-4-5-20251001", &usage), Some(6.0));
        assert_eq!(cost_usd("claude-opus-5-5", &usage), Some(24.0));
        assert_eq!(cost_usd("claude-opus-5", &usage), Some(30.0));
        assert_eq!(cost_usd("gpt-4", &usage), None);
    }

    #[test]
    fn parses_api_response() {
        let body = json!({
            "model": "claude-opus-5",
            "stop_reason": "tool_use",
            "content": [{"type": "text", "text": "Reading."}, {"type": "tool_use", "id": "t1", "name": "read", "input": {"p": 1}}],
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 2}
        });
        let r = parse_response(&body).unwrap();
        assert_eq!(r.text(), "Reading.");
        assert_eq!(r.tool_uses()[0].name, "read");
        assert_eq!(r.usage.total_tokens(), 17);
    }

    #[test]
    fn a_cassette_replays_what_it_recorded_in_any_order() {
        let dir = std::env::temp_dir().join(format!("grenat-cassette-{}", std::process::id()));
        let path = dir.join("calls.json");
        let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
        let ask = |text: &str| Request {
            model: &model,
            system: None,
            messages: vec![json!({"role": "user", "content": text})],
            tools: Vec::new(),
            output_schema: None,
        };
        let real = std::sync::Arc::new(Scripted::responder(|body| {
            Response::text_reply(format!("echo {}", body["messages"][0]["content"].as_str().unwrap()))
        }));
        let recorder = Cassette::record(&path, real.clone());
        assert!(recorder.is_recording());
        assert_eq!(recorder.complete(&ask("a")).unwrap().text(), "echo a");
        assert_eq!(recorder.complete(&ask("b")).unwrap().text(), "echo b");
        recorder.save().unwrap();

        let player = Cassette::replay(&path).unwrap();
        assert_eq!(player.complete(&ask("b")).unwrap().text(), "echo b");
        assert_eq!(player.complete(&ask("a")).unwrap().text(), "echo a");
        let missing = player.complete(&ask("c")).unwrap_err();
        assert!(missing.message.contains("GRENAT_RECORD=1"), "{}", missing.message);
        // the real provider was only called while recording
        assert_eq!(real.requests().len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_mock_shapes_its_answers_for_each_request() {
        let model = ModelConfig::new("anthropic", "claude-haiku-4-5");
        let request = |tools: Vec<ToolSpec>, output_schema: Option<serde_json::Value>| Request {
            model: &model,
            system: None,
            messages: vec![json!({"role": "user", "content": "hi"})],
            tools,
            output_schema,
        };
        let wrapped = json!({"type": "object", "properties": {"value": {"type": "integer"}}});
        let final_tool = ToolSpec { name: "final_answer".into(), description: String::new(), input_schema: wrapped.clone(), strict: true };
        let mock = Mock::new("`:fast`", [
            MockReply::Answer(json!("plain")),
            MockReply::Answer(json!(42)),
            MockReply::Answer(json!({"title": "t"})),
            MockReply::Tool { name: "search".into(), input: json!({"q": "x"}) },
            MockReply::Answer(json!(7)),
            MockReply::Error("overloaded".into()),
        ]);
        assert_eq!(mock.complete(&request(vec![], None)).unwrap().text(), "plain");
        assert_eq!(mock.complete(&request(vec![], Some(wrapped.clone()))).unwrap().text(), r#"{"value":42}"#);
        let object = json!({"type": "object", "properties": {"title": {"type": "string"}}});
        assert_eq!(mock.complete(&request(vec![], Some(object))).unwrap().text(), r#"{"title":"t"}"#);
        let tool = mock.complete(&request(vec![final_tool.clone()], None)).unwrap();
        assert_eq!(tool.tool_uses()[0].name, "search");
        let last = mock.complete(&request(vec![final_tool], None)).unwrap();
        assert_eq!(last.tool_uses()[0].name, "final_answer");
        assert_eq!(last.tool_uses()[0].input, json!({"value": 7}));
        assert_eq!(last.usage.total_tokens(), 0);
        assert_eq!(mock.complete(&request(vec![], None)).unwrap_err().message, "overloaded");
        assert_eq!(mock.remaining(), 0);
        let empty = mock.complete(&request(vec![], None)).unwrap_err();
        assert_eq!(empty.message, "the mock of `:fast` has no reply left");
    }
}
