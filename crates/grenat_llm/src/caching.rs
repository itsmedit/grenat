//! Prompt caching on Anthropic's Messages API: where a request's
//! `cache_control` breakpoints go.
//!
//! The cache keeps a request's prefix — tools, then system prompt, then
//! messages — up to each breakpoint, and the next request starting with the
//! same bytes reads it at a fraction of the price. A breakpoint goes where
//! what follows changes from one request to the next:
//!
//! - an agent's turns (the requests giving tools): after the last tool,
//!   after the instructions, and after the last message — each turn sends
//!   the whole run again, and reads all of it but what the last turn added;
//! - other requests, with `cache: true`: after the tools and the system
//!   prompt, after the history before the last message (a conversation),
//!   and after the documents and images the last message gives before its
//!   question (a prompt over the same document).
//!
//! The API takes four breakpoints at most; a request gets three at most. A
//! prefix shorter than the model's minimum (512 to 4,096 tokens) is not
//! cached, and costs nothing more. OpenAI and the other providers cache by
//! themselves: nothing is marked for them.

use serde_json::{Value as Json, json};

use crate::*;

/// The most breakpoints a request may carry (the API refuses more).
pub(crate) const MAX_BREAKPOINTS: usize = 4;

/// Content blocks that may carry a breakpoint: not thinking, not an empty text.
const CACHEABLE: [&str; 6] = ["text", "image", "document", "tool_use", "tool_result", "search_result"];

/// Marks `body`, the Messages API request built for `request`.
pub(crate) fn mark(body: &mut Json, request: &Request) {
    let agent = !request.tools.is_empty();
    match request.model.cache {
        Caching::Off => return,
        Caching::Agents if !agent => return,
        Caching::Agents | Caching::Always => {}
    }
    let mut marks =
        Marks { marker: marker(request.model.cache_ttl), left: MAX_BREAKPOINTS.saturating_sub(count(body)) };
    if let Some(last) = body["tools"].as_array_mut().and_then(|tools| tools.last_mut()) {
        marks.put(last);
    }
    if let Some(system) = body["system"].as_str().filter(|s| !s.is_empty()).map(String::from) {
        let mut block = json!({"type": "text", "text": system});
        marks.put(&mut block);
        body["system"] = json!([block]);
    }
    let Some(messages) = body["messages"].as_array_mut() else { return };
    let Some((last, history)) = messages.split_last_mut() else { return };
    if agent {
        marks.put_last(last, |_| true);
        return;
    }
    if let Some(previous) = history.last_mut() {
        marks.put_last(previous, |_| true);
    }
    // attachments come before the question: the last one ends what repeats
    if blocks(last).iter().any(|b| b["type"] != "text") {
        marks.put_last(last, |block| block["type"] != "text");
    }
}

/// The `cache_control` of a breakpoint.
fn marker(ttl: CacheTtl) -> Json {
    match ttl {
        CacheTtl::FiveMinutes => json!({"type": "ephemeral"}),
        CacheTtl::OneHour => json!({"type": "ephemeral", "ttl": "1h"}),
    }
}

/// Breakpoints still allowed, and how they are written.
struct Marks {
    marker: Json,
    left: usize,
}

impl Marks {
    fn put(&mut self, block: &mut Json) {
        if self.left > 0 && block.get("cache_control").is_none() {
            block["cache_control"] = self.marker.clone();
            self.left -= 1;
        }
    }

    /// On the last block of `message` that may carry one and is `wanted`.
    fn put_last(&mut self, message: &mut Json, wanted: impl Fn(&Json) -> bool) {
        if let Json::String(text) = &message["content"] {
            if text.is_empty() {
                return;
            }
            message["content"] = json!([{"type": "text", "text": text}]);
        }
        let Some(blocks) = message["content"].as_array_mut() else { return };
        if let Some(block) = blocks.iter_mut().rev().find(|b| cacheable(b) && wanted(b)) {
            self.put(block);
        }
    }
}

fn cacheable(block: &Json) -> bool {
    let kind = block["type"].as_str().unwrap_or_default();
    CACHEABLE.contains(&kind) && !(kind == "text" && block["text"].as_str().is_none_or(str::is_empty))
}

fn blocks(message: &Json) -> &[Json] {
    message["content"].as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The breakpoints `json` already has.
pub(crate) fn count(json: &Json) -> usize {
    match json {
        Json::Object(map) => usize::from(map.contains_key("cache_control")) + map.values().map(count).sum::<usize>(),
        Json::Array(items) => items.iter().map(count).sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(cache: Caching, cache_ttl: CacheTtl) -> ModelConfig {
        ModelConfig { cache, cache_ttl, ..ModelConfig::new("anthropic", "claude-haiku-4-5") }
    }

    fn tool(name: &str) -> ToolSpec {
        ToolSpec { name: name.into(), description: "d".into(), input_schema: json!({"type": "object"}), strict: true }
    }

    fn body(model: &ModelConfig, tools: Vec<ToolSpec>, messages: Vec<Json>) -> Json {
        request_body(&Request { model, system: Some("sys".into()), messages, tools, output_schema: None })
    }

    /// A five-minute breakpoint.
    fn five() -> Json {
        json!({"type": "ephemeral"})
    }

    #[test]
    fn an_agent_turn_caches_its_tools_instructions_and_history() {
        let agent = model(Caching::Agents, CacheTtl::FiveMinutes);
        let history = vec![
            json!({"role": "user", "content": "Question"}),
            json!({"role": "assistant", "content": [{"type": "thinking", "thinking": "", "signature": "s"}, {"type": "tool_use", "id": "t1", "name": "read", "input": {}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "a"}, {"type": "tool_result", "tool_use_id": "t2", "content": "b"}]}),
        ];
        let b = body(&agent, vec![tool("read"), tool("final_answer")], history);
        assert!(b["tools"][0].get("cache_control").is_none());
        assert_eq!(b["tools"][1]["cache_control"], five());
        assert_eq!(b["system"], json!([{"type": "text", "text": "sys", "cache_control": five()}]));
        assert_eq!(b["messages"][2]["content"][1]["cache_control"], five());
        assert_eq!(count(&b["messages"]), 1, "only the end of the history");
        assert_eq!(count(&b), 3);
        // the first turn: its instruction becomes a block
        let first = body(&agent, vec![tool("final_answer")], vec![json!({"role": "user", "content": "Question"})]);
        assert_eq!(
            first["messages"][0]["content"],
            json!([{"type": "text", "text": "Question", "cache_control": five()}])
        );
    }

    #[test]
    fn by_default_prompts_and_conversations_are_left_alone() {
        let agent = model(Caching::Agents, CacheTtl::FiveMinutes);
        let b = body(&agent, Vec::new(), vec![json!({"role": "user", "content": "hi"})]);
        assert_eq!(b["system"], "sys");
        assert_eq!(count(&b), 0);
        let off = model(Caching::Off, CacheTtl::FiveMinutes);
        let b = body(&off, vec![tool("t")], vec![json!({"role": "user", "content": "hi"})]);
        assert_eq!((count(&b), b["system"].as_str()), (0, Some("sys")));
    }

    #[test]
    fn with_cache_true_what_repeats_is_cached_never_the_question() {
        let always = model(Caching::Always, CacheTtl::OneHour);
        let hour = json!({"type": "ephemeral", "ttl": "1h"});
        // a prompt over a document: the document, not the question
        let document =
            json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "AA=="}});
        let b = body(
            &always,
            Vec::new(),
            vec![json!({"role": "user", "content": [document, {"type": "text", "text": "What is the total?"}]})],
        );
        assert_eq!(b["system"][0]["cache_control"], hour);
        assert_eq!(b["messages"][0]["content"][0]["cache_control"], hour);
        assert!(b["messages"][0]["content"][1].get("cache_control").is_none());
        assert_eq!(count(&b), 2);
        // a question alone: the system prompt only
        let b = body(&always, Vec::new(), vec![json!({"role": "user", "content": "What is the total?"})]);
        assert_eq!((count(&b), b["messages"][0]["content"].as_str()), (1, Some("What is the total?")));
        // a conversation: the history before the last message
        let turns = vec![
            json!({"role": "user", "content": "I am Ada"}),
            json!({"role": "assistant", "content": "Hello Ada"}),
            json!({"role": "user", "content": "Who am I?"}),
        ];
        let b = body(&always, Vec::new(), turns);
        assert_eq!(b["messages"][1]["content"], json!([{"type": "text", "text": "Hello Ada", "cache_control": hour}]));
        assert_eq!(b["messages"][2]["content"], "Who am I?");
        assert_eq!(count(&b), 2);
    }

    #[test]
    fn never_more_breakpoints_than_the_api_takes() {
        let always = model(Caching::Always, CacheTtl::FiveMinutes);
        let marked = |text: &str| json!({"type": "text", "text": text, "cache_control": {"type": "ephemeral"}});
        let image = json!({"type": "image", "source": {"type": "url", "url": "https://x/y.png"}});
        let messages = vec![
            json!({"role": "user", "content": [marked("a"), marked("b")]}),
            json!({"role": "assistant", "content": "c"}),
            json!({"role": "user", "content": [image, {"type": "text", "text": "q"}]}),
        ];
        let b = body(&always, vec![tool("t")], messages);
        assert_eq!(count(&b), MAX_BREAKPOINTS);
        assert_eq!(b["tools"][0]["cache_control"], five());
        assert_eq!(b["system"][0]["cache_control"], five());
        assert_eq!(b["messages"][1]["content"], "c", "no breakpoint left for it");
    }

    #[test]
    fn empty_texts_and_thinking_carry_none() {
        let agent = model(Caching::Agents, CacheTtl::FiveMinutes);
        let b = body(&agent, vec![tool("t")], vec![json!({"role": "user", "content": ""})]);
        assert_eq!(b["messages"][0]["content"], "");
        let thinking =
            json!({"role": "user", "content": [{"type": "text", "text": ""}, {"type": "thinking", "thinking": "x"}]});
        let b = body(&agent, vec![tool("t")], vec![thinking.clone()]);
        assert_eq!(b["messages"][0], thinking);
    }
}
