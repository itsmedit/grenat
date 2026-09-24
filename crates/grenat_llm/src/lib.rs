//! Accès aux modèles de langage.
//!
//! Le runtime ne dépend que du trait [`Provider`] : l'implémentation réelle
//! ([`Anthropic`]) parle HTTP à l'API Messages, [`Scripted`] rejoue des
//! réponses préparées pour les tests.

mod anthropic;
mod pricing;
mod scripted;
mod types;
mod wire;

pub use anthropic::Anthropic;
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
            "content": [{"type": "text", "text": "Je lis."}, {"type": "tool_use", "id": "t1", "name": "read", "input": {"p": 1}}],
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 2}
        });
        let r = parse_response(&body).unwrap();
        assert_eq!(r.text(), "Je lis.");
        assert_eq!(r.tool_uses()[0].name, "read");
        assert_eq!(r.usage.total_tokens(), 17);
    }
}
