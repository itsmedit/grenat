//! `Conversation`: history, compaction into a summary, persistence.

mod common;

use common::*;
use grenat_interp::{Response, Scripted};

const HEAD: &str = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\n";

#[test]
fn the_history_is_sent_and_older_turns_are_compacted() {
    let src = format!(
        "{HEAD}chat = Conversation.new(model: :fast, system: \"Be brief.\", keep: 2)
p chat.say(\"I am Ada\")
p chat.say(\"I like Rust\")
p chat.say(\"Who am I?\")
p chat.history.size, chat.summary
p chat.say(\"And what do I like?\")
"
    );
    // answers computed from the request: the last user message, echoed
    let provider = Scripted::responder(|body| {
        let messages = body["messages"].as_array().unwrap();
        let last = messages.last().unwrap()["content"].as_str().unwrap();
        if body["system"].as_str().unwrap_or_default().starts_with("You compact") {
            return Response::text_reply("The user is Ada.");
        }
        Response::text_reply(format!("re: {last}"))
    });
    let r = run_provider(&src, provider, &[], &[]);
    let requests = r.requests.clone();
    let out = r.ok();
    assert_eq!(
        out,
        "~\"re: I am Ada\"\n~\"re: I like Rust\"\n~\"re: Who am I?\"\n4\n~\"The user is Ada.\"\n~\"re: And what do I like?\"\n"
    );
    // the second call sends the first exchange
    assert_eq!(requests[1]["messages"].as_array().unwrap().len(), 3);
    assert_eq!(requests[1]["system"], "Be brief.");
    // after 3 exchanges (keep: 2), the oldest one is compacted
    assert!(requests[3]["messages"][0]["content"].as_str().unwrap().contains("user: I am Ada\nassistant: re: I am Ada\n"));
    // then the summary goes with the system prompt, and the old turns do not
    assert_eq!(requests[4]["system"], "Be brief.\n\nEarlier in this conversation:\nThe user is Ada.");
    assert_eq!(requests[4]["messages"].as_array().unwrap().len(), 5);
}

#[test]
fn a_conversation_is_saved_and_loaded() {
    let dir = temp_dir("conversation");
    let path = dir.join("memory.json");
    let src = format!(
        "{HEAD}chat = Conversation.load(\"{0}\", model: :fast)\np chat.history.size\nchat.say(\"hi\")\nchat.save(\"{0}\")\n",
        path.display()
    );
    let reply = || Scripted::new([Response::text_reply("hello")]);
    assert_eq!(run_provider(&src, reply(), &[], &[]).ok(), "0\n");
    assert_eq!(run_provider(&src, reply(), &[], &[]).ok(), "2\n");
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["turns"].as_array().unwrap().len(), 4);
    let e = run_err(&format!("{HEAD}Conversation.new(keep: 1)\n"), Vec::new());
    assert_eq!(e.message, "invalid `Conversation` option `keep: 1`");
}
