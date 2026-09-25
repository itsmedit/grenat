//! Test doubles for model-driven code: `mock` (the model's replies, given as
//! values), `cassette` (real calls recorded once, then replayed), `fixture`
//! (test data files) and `call` (a mocked reply calling a tool).
//!
//! `cassettes/` and `fixtures/` are found in the program's directory.

use std::path::PathBuf;

use grenat_llm::{Anthropic, Cassette, Mock, MockReply, ModelConfig, Provider};

use crate::builtins::json_to_untyped;
use crate::llm::value_to_json;
use crate::prelude::*;

impl<'p> Interp<'p> {
    // ── mock ──────────────────────────────────────────────────

    /// `mock :fast, replies: [...]` (or any model, without a name): the next
    /// calls to that model get these replies, in order.
    pub(crate) fn mock(&mut self, model: Option<&str>, replies: &Value<'p>) -> R<'p> {
        let Value::Array(items) = replies.untainted() else {
            return raise("TypeError", "`mock` expects `replies: [...]`");
        };
        let replies: Vec<MockReply> = items.borrow().iter().map(mock_reply).collect();
        let (config, label) = match model {
            Some(name) => (Some(self.model_named(name)?), format!("`:{name}`")),
            None => (None, "every model".to_string()),
        };
        let mock = Arc::new(Mock::new(label, replies));
        // the latest mock of a model replaces the previous one
        let mut mocks = self.mocks.borrow_mut();
        mocks.retain(|(m, _)| *m != config);
        mocks.push((config, mock));
        Ok(Value::Nil)
    }

    /// The mock answering for `model`, if any: its own, else the catch-all one.
    pub(crate) fn mock_for(&self, model: &ModelConfig) -> Option<Arc<dyn Provider>> {
        let mocks = self.mocks.borrow();
        let own = mocks.iter().find(|(m, _)| m.as_ref() == Some(model));
        let any = || mocks.iter().find(|(m, _)| m.is_none());
        own.or_else(any).map(|(_, mock)| mock.clone() as Arc<dyn Provider>)
    }

    // ── cassette ──────────────────────────────────────────────

    /// `cassette "name" do … end`: the calls of the block are replayed from
    /// `cassettes/<name>.json`, or recorded there when it does not exist yet
    /// (or with `GRENAT_RECORD=1`). A recording is kept only if the block succeeds.
    pub(crate) fn cassette(&mut self, name: &str, body: &Value<'p>) -> R<'p> {
        let path = self.dir.join("cassettes").join(format!("{name}.json"));
        let cassette = if self.record || !path.exists() {
            Cassette::record(&path, self.recorder(name)?)
        } else {
            Cassette::replay(&path).or_else(|e| raise("LlmError", e.message))?
        };
        let cassette = Arc::new(cassette);
        self.providers.push(cassette.clone());
        let result = self.call_block(body, Vec::new());
        self.providers.pop();
        if result.is_ok()
            && let Err(e) = cassette.save()
        {
            return raise("IOError", format!("cannot save the cassette {}: {e}", path.display()));
        }
        if result.is_ok() && cassette.is_recording() && self.log {
            self.write_err(&format!("[cassette] {name}: recorded\n"));
        }
        result
    }

    /// The real provider a cassette records (offline runs included: recording
    /// is what makes them deterministic afterwards).
    fn recorder(&self, name: &str) -> Result<Arc<dyn Provider>, Ctrl<'p>> {
        if let Some(provider) = self.provider.borrow().clone() {
            return Ok(provider);
        }
        match Anthropic::from_env() {
            Ok(client) => Ok(Arc::new(client)),
            Err(e) => raise("LlmError", format!("cannot record the cassette `{name}`: {}", e.message)),
        }
    }

    // ── fixture ───────────────────────────────────────────────

    /// `fixture("reviews.json")`: a file of `fixtures/`, as data for `.json`
    /// and `.jsonl` (one value per line), as text otherwise.
    pub(crate) fn fixture(&self, name: &str) -> R<'p> {
        let path: PathBuf = self.dir.join("fixtures").join(name);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => return raise("IOError", format!("cannot read the fixture {}: {e}", path.display())),
        };
        let parse = |text: &str| {
            serde_json::from_str::<serde_json::Value>(text)
                .map(|json| json_to_untyped(&json))
                .or_else(|e| raise("ParseError", format!("invalid JSON in the fixture {}: {e}", path.display())))
        };
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => parse(&text),
            Some("jsonl") => {
                let rows = text.lines().filter(|l| !l.trim().is_empty()).map(parse).collect::<Result<_, _>>()?;
                Ok(Value::array(rows))
            }
            _ => Ok(Value::str(text)),
        }
    }

    // ── call ──────────────────────────────────────────────────

    /// `call(:tool, arg: value…)`: a reply of a mocked model calling one of
    /// the agent's tools (its arguments then go through the tool's schema).
    pub(crate) fn tool_call_reply(&self, tool: &str, args: &Args<'p>) -> R<'p> {
        match self.fns.get(tool) {
            Some(def) if def.kind == FnKind::Tool => {}
            Some(_) => return raise("TypeError", format!("`{tool}` is not a `tool`")),
            None => return raise("NameError", format!("unknown tool `{tool}`")),
        }
        let input = args.named.iter().map(|(name, value)| (Value::str(name), value.clone())).collect();
        let fields = vec![
            ("tool".into(), Value::Symbol(tool.into())),
            ("input".into(), Value::Hash(Arc::new(Mutex::new(input)))),
        ];
        Ok(Value::record(TOOL_CALL, fields))
    }
}

/// What `call` builds.
const TOOL_CALL: &str = "ToolCall";

/// A reply given to `mock`: an error fails the call, a `call(:tool, …)`
/// calls one of the agent's tools, anything else is the answer.
fn mock_reply(value: &Value) -> MockReply {
    match value.untainted() {
        Value::Error(e) => MockReply::Error(e.message.clone()),
        Value::Record(r) if &*r.ty == TOOL_CALL => {
            let get = |name: &str| r.fields.iter().find(|(k, _)| &**k == name).map(|(_, v)| v);
            MockReply::Tool {
                name: get("tool").map(Value::to_display).unwrap_or_default(),
                input: get("input").map(value_to_json).unwrap_or_default(),
            }
        }
        _ => MockReply::Answer(value_to_json(value)),
    }
}
