//! Repository examples run end to end.

mod common;

use common::*;
use grenat_interp::Response;
use grenat_interp::Scripted;
use serde_json::json;

#[test]
fn explorer_example_runs_end_to_end() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/explorer.grn")).unwrap();
    let replies = vec![
        Response::tool_call("t1", "list_dir", json!({"path": "src"})),
        Response::tool_call("t2", "read_file", json!({"path": "src/lib.rs", "max_lines": 1})),
        Response::tool_call(
            "t3",
            "final_answer",
            json!({"summary": "An interpreter.", "files_read": ["src/lib.rs"], "maturity": "Solid", "next_steps": ["Add types"]}),
        ),
        Response::text_reply("Grenat: typed agents at last."),
    ];
    let r = run_with(&src, replies, &[]);
    r.result.unwrap();
    assert_eq!(
        r.output,
        "## arg1 — Solid\nAn interpreter.\n\nFiles read: src/lib.rs\n- Add types\n\n> Grenat: typed agents at last.\n"
    );
    let listing = &r.requests[1]["messages"][2]["content"][0]["content"];
    assert!(listing.as_str().unwrap().contains("lib.rs"), "{listing}");
    let first_line = r.requests[2]["messages"][4]["content"][0]["content"].as_str().unwrap().to_string();
    let expected = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")).unwrap();
    assert_eq!(first_line, expected.lines().next().unwrap());
    assert_eq!(r.requests[0]["fallbacks"], "default");
    assert_eq!(r.requests[3]["model"], "claude-haiku-4-5");
}

#[test]
fn basics_example_runs() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/basics.grn")).unwrap();
    assert!(run(&src).starts_with("fib(25) = 75025\n"));
}

#[test]
fn support_desk_example_runs_end_to_end() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/support_desk.grn")).unwrap();
    // both tickets are handled in parallel: replies are computed from the request
    let provider = Scripted::responder(|body| {
        if body.get("tools").is_some() {
            return Response::tool_call(
                "t1",
                "final_answer",
                json!({"body": "Menu Invoices > Export.", "sources": ["docs/export.md"], "confidence": 0.9}),
            );
        }
        let spam = body["messages"][0]["content"].as_str().unwrap().contains("WIN");
        let category = if spam { "Spam" } else { "Question" };
        Response::json_reply(json!({"category": category, "priority": "Normal", "language": "en", "reason": "reason"}))
    });
    // offline, as in tests: the reply is kept, not emailed
    let mode = Mode { offline: true, ..Mode::default() };
    let r = run_mode(&src, provider, &["y"], &["../../examples/tickets.jsonl"], mode);
    if let Err(e) = &r.result {
        panic!("{e:?}\n{}", r.output);
    }
    assert!(r.output.contains("Send to ana@example.com?"), "{}", r.output);
    assert!(r.output.ends_with("✓ 1 sent, 1 skipped — cost $0.0014\n"), "{}", r.output);
    assert_eq!(r.requests.len(), 3);
    assert_eq!(r.requests.iter().filter(|q| q["model"] == "claude-opus-5").count(), 1);
}

#[test]
fn reviews_example_runs_end_to_end() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/reviews.grn")).unwrap();
    // the reviews are analysed in parallel: answers are computed from the request
    let provider = Scripted::responder(|body| {
        let review = body["messages"][0]["content"].as_str().unwrap().to_string();
        let (sentiment, topic) = if review.contains("croissant") {
            ("negative", "service")
        } else if review.contains("loud") || review.contains("pricey") {
            ("neutral", "price")
        } else {
            ("positive", "coffee")
        };
        let summary = review.split(',').next().unwrap().trim_end_matches('.').to_string();
        Response::json_reply(json!({"sentiment": sentiment, "topics": [topic], "summary": summary}))
    });
    let r = run_provider(&src, provider, &[], &[]);
    if let Err(e) = &r.result {
        panic!("{e:?}\n{}", r.output);
    }
    let expected = "\
5★ ██ 2
4★ █ 1
3★ █ 1
2★ █ 1
1★  0
average: 3.8 / 5
positive Best flat white in the area  [coffee]
neutral  Great coffee  [price]
negative Waited 20 minutes for a cold croissant. Not coming back  [service]
positive Cosy corner to work  [coffee]
neutral  Good espresso  [price]
";
    assert!(r.output.starts_with(expected), "{}", r.output);
    assert!(r.output.contains("cost: $"), "{}", r.output);
    assert_eq!(r.requests.len(), 5);
}
