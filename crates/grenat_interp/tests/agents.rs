//! Agents: the `run` loop, tools, tool errors, messages.

mod common;

use common::*;
use grenat_interp::Response;
use serde_json::json;

#[test]
fn agent_runs_the_tool_loop_until_final_answer() {
    let src = format!(
        "{READER}r = spawn Reader\nrep = r.ask(Ask(question: \"what is in a.txt?\"))\nputs rep.answer\np rep.files, r.ask(Asked())\n"
    );
    let replies = vec![
        Response::tool_call("t1", "read_file", json!({"path": "a.txt", "max_lines": null})),
        Response::tool_call("t2", "final_answer", json!({"answer": "some text", "files": ["a.txt"]})),
    ];
    let r = run_with(&src, replies, &[]);
    r.result.unwrap();
    assert_eq!(r.output, "some text\n~[\"a.txt\"]\n1\n");

    let requests = r.requests;
    assert_eq!(requests.len(), 2);
    let first = &requests[0];
    assert!(first["system"].as_str().unwrap().starts_with("You read files."));
    assert_eq!(first["messages"][0]["content"], "Question: what is in a.txt?");
    let tools = first["tools"].as_array().unwrap();
    assert_eq!(tools[0]["name"], "read_file");
    assert_eq!(tools[0]["description"], "Reads a file.");
    assert_eq!(tools[0]["strict"], true);
    assert_eq!(tools[0]["input_schema"]["properties"]["max_lines"]["anyOf"][1]["type"], "null");
    assert_eq!(tools[1]["name"], "final_answer");

    let second = &requests[1]["messages"];
    assert_eq!(second[1]["role"], "assistant");
    let result = &second[2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "t1");
    assert_eq!(result["content"], "contents of a.txt (10 lines)");
    assert_eq!(result["is_error"], false);
}

#[test]
fn tool_errors_are_reported_to_the_model() {
    let src = format!("{READER}r = spawn Reader\nputs r.ask(Ask(question: \"?\")).answer\n");
    let replies = vec![
        Response::tool_call("t1", "read_file", json!({"path": 42, "max_lines": null})),
        Response::tool_call("t2", "final_answer", json!({"answer": "ok", "files": []})),
    ];
    let r = run_with(&src, replies, &[]);
    r.result.unwrap();
    let result = &r.requests[1]["messages"][2]["content"][0];
    assert_eq!(result["is_error"], true);
    assert!(result["content"].as_str().unwrap().contains("expected a string"), "{result}");
}

#[test]
fn agent_gives_up_after_max_turns() {
    let src = format!("{READER}r = spawn Reader\nr.ask(Ask(question: \"?\"))\n");
    let replies = (0..4).map(|_| Response::text_reply("thinking")).collect();
    let e = run_err(&src, replies);
    assert_eq!(e.ty, "MaxTurnsExceeded");
}

#[test]
fn unknown_message_is_an_error() {
    let src = format!("{READER}r = spawn Reader\nr.ask(Asked(1))\n");
    let e = run_err(&src, vec![]);
    assert_eq!(e.ty, "ArgumentError");
}
