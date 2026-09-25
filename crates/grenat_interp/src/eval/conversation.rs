//! Conversations: a history of turns with a model, compacted as it grows.
//!
//! ```ruby
//! chat = Conversation.new(model: :fast, system: "You are a helpful assistant.", keep: 10)
//! chat.say("I am Ada")        # the answer (untrusted); the history is sent every time
//! chat.history                # [{"role" => "user", "text" => …}, …]
//! chat.summary                # what older turns were compacted into
//! chat.save("memory.json")    # Conversation.load("memory.json", model: :fast, …)
//! ```
//!
//! Beyond `keep` exchanges, the older half is summarized by the model and
//! dropped: the summary goes with the system prompt, the context stays small.

use grenat_llm::{ModelConfig, Request};
use serde_json::{Value as Json, json};

use crate::builtins::{arg, str_arg};
use crate::prelude::*;

/// The record `Conversation.new` returns.
pub(crate) const CONVERSATION: &str = "Conversation";
const DEFAULT_KEEP: usize = 20;

#[derive(Clone)]
pub(crate) struct ConversationState {
    model: ModelConfig,
    system: String,
    /// Exchanges kept before compacting.
    keep: usize,
    /// (role, text), oldest first.
    turns: Vec<(String, String)>,
    summary: String,
}

impl<'p> Interp<'p> {
    /// `Conversation.new(model:, system:, keep:)` and `Conversation.load(path, …)`.
    pub(crate) fn conversation_static(&mut self, name: &str, args: &Args<'p>) -> R<'p> {
        let mut state = ConversationState {
            model: self.model(None)?,
            system: String::new(),
            keep: DEFAULT_KEEP,
            turns: Vec::new(),
            summary: String::new(),
        };
        for (option, value) in &args.named {
            match (option.as_str(), value.untainted()) {
                ("model", Value::Symbol(m)) => state.model = self.model_named(m)?,
                ("system", v) => state.system = v.to_display(),
                ("keep", Value::Int(n)) if *n >= 2 => state.keep = *n as usize,
                (option, v) => {
                    return raise("ArgumentError", format!("invalid `Conversation` option `{option}: {}`", v.inspect()));
                }
            }
        }
        match name {
            "new" => {}
            "load" => {
                let path = str_arg(args, 0, name)?.to_string();
                self.check_fs("fs.read", &path)?;
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let saved: Json =
                            serde_json::from_str(&text).or_else(|e| raise("ParseError", format!("{path}: {e}")))?;
                        state.summary = saved["summary"].as_str().unwrap_or_default().to_string();
                        state.turns = saved["turns"]
                            .as_array()
                            .map(|turns| {
                                turns
                                    .iter()
                                    .map(|t| (t["role"].as_str().unwrap_or("user").to_string(), t["text"].as_str().unwrap_or_default().to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                    }
                    // nothing saved yet: a new conversation
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return raise("IoError", format!("reading `{path}`: {e}")),
                }
            }
            _ => return raise("NoMethodError", format!("unknown method `Conversation.{name}`")),
        }
        let mut conversations = self.conversations.borrow_mut();
        conversations.push(Arc::new(Mutex::new(state)));
        Ok(Value::record(CONVERSATION, vec![("id".into(), Value::Int(conversations.len() as i64 - 1))]))
    }

    pub(crate) fn conversation_method(&mut self, fields: &Fields<'p>, name: &str, args: &Args<'p>) -> R<'p> {
        let Some(Value::Int(id)) = fields.first().map(|(_, v)| v.clone()) else {
            return raise("TypeError", "not a conversation");
        };
        let shared = self.conversations.borrow()[id as usize].clone();
        let state = shared.borrow().clone();
        match name {
            "say" => {
                let message = arg(args, 0, name)?.to_display();
                let (reply, state) = self.say(state, message)?;
                *shared.borrow_mut() = state;
                Ok(Value::str(reply).taint())
            }
            "history" => {
                let turns = state.turns.iter().map(|(role, text)| {
                    let pairs = vec![(Value::str("role"), Value::str(role)), (Value::str("text"), Value::str(text))];
                    Value::Hash(Arc::new(Mutex::new(pairs)))
                });
                Ok(Value::array(turns.collect()))
            }
            // written by the model
            "summary" => Ok(Value::str(&state.summary).taint()),
            "save" => {
                let path = str_arg(args, 0, name)?.to_string();
                self.check_fs("fs.write", &path)?;
                let turns: Vec<Json> = state.turns.iter().map(|(role, text)| json!({"role": role, "text": text})).collect();
                let saved = json!({"summary": state.summary, "turns": turns});
                std::fs::write(&path, saved.to_string()).or_else(|e| raise("IoError", format!("writing `{path}`: {e}")))?;
                Ok(Value::Nil)
            }
            _ => raise("NoMethodError", format!("unknown method `{name}` for a conversation")),
        }
    }

    /// One exchange, then compaction if the history grew beyond `keep`.
    fn say(&mut self, mut state: ConversationState, message: String) -> Result<(String, ConversationState), Ctrl<'p>> {
        state.turns.push(("user".into(), message));
        let reply = {
            let request = Request {
                model: &state.model,
                system: system(&state),
                messages: state.turns.iter().map(|(role, text)| json!({"role": role, "content": text})).collect(),
                tools: Vec::new(),
                output_schema: None,
            };
            self.llm_call(&request)?.text()
        };
        state.turns.push(("assistant".into(), reply.clone()));
        if state.turns.len() > state.keep * 2 {
            // the older half, as whole exchanges
            let older: Vec<(String, String)> = state.turns.drain(..state.keep / 2 * 2).collect();
            let transcript: String = older.iter().map(|(role, text)| format!("{role}: {text}\n")).collect();
            let request = Request {
                model: &state.model,
                system: Some("You compact conversations: keep every fact about the user and every decision.".into()),
                messages: vec![json!({"role": "user", "content": format!(
                    "Summary so far:\n{}\n\nNew turns:\n{transcript}\nWrite the updated summary, in at most 10 lines.",
                    if state.summary.is_empty() { "(none)" } else { &state.summary }
                )})],
                tools: Vec::new(),
                output_schema: None,
            };
            state.summary = self.llm_call(&request)?.text();
        }
        Ok((reply, state))
    }
}

fn system(state: &ConversationState) -> Option<String> {
    let mut parts = Vec::new();
    if !state.system.is_empty() {
        parts.push(state.system.clone());
    }
    if !state.summary.is_empty() {
        parts.push(format!("Earlier in this conversation:\n{}", state.summary));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}
