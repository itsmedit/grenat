//! MCP servers: tools handed to agents, approval, direct calls, capabilities, mocks.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Response, run_tests};
use serde_json::json;

fn agent(tools: &str, server_options: &str) -> String {
    format!(
        "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
mcp :fake, url: \"{}\"{server_options}
agent Clerk
  model :fast
  tools {tools}
  on Go -> String
    run \"go\"
  end
end
puts spawn(Clerk).ask(Go())
",
        grenat_mcp::fake::serve_http()
    )
}

fn final_answer(text: &str) -> Response {
    Response::tool_call("f", "final_answer", json!({"value": text}))
}

#[test]
fn an_agent_uses_a_server_s_tools() {
    let replies = vec![
        Response::tool_call("t1", "fake__echo", json!({"text": "hi"})),
        final_answer("done"),
    ];
    let r = run_with(&agent("mcp(:fake)", ""), replies, &[]);
    let requests = r.requests.clone();
    assert_eq!(r.ok(), "done\n");
    let tools = requests[0]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["fake__echo", "fake__add", "fake__delete_all", "final_answer"]);
    assert_eq!(tools[0]["description"], "[fake] Echoes a text.");
    assert_eq!(tools[0]["strict"], false);
    assert_eq!(tools[3]["strict"], true);
    let result = &requests[1]["messages"][2]["content"][0];
    assert_eq!((result["content"].as_str(), result["is_error"].as_bool()), (Some("hi"), Some(false)));
}

#[test]
fn only_some_tools() {
    let r = run_with(&agent("mcp(:fake, only: [\"echo\"])", ""), vec![final_answer("ok")], &[]);
    let requests = r.requests.clone();
    r.ok();
    assert_eq!(requests[0]["tools"].as_array().unwrap().len(), 2);
    let e = run_err(&agent("mcp(:fake, only: [\"nope\"])", ""), vec![final_answer("ok")]);
    assert_eq!(e.message, "the MCP server `:fake` has no tool `nope`");
}

#[test]
fn tools_that_change_things_are_approved_first() {
    let replies = || {
        vec![
            Response::tool_call("t1", "fake__delete_all", json!({})),
            Response::tool_call("t2", "fake__add", json!({"a": 1, "b": 2})),
            final_answer("done"),
        ]
    };
    // denied, then allowed: the model is told
    let r = run_with(&agent("mcp(:fake)", ""), replies(), &["n", "y"]);
    let requests = r.requests.clone();
    let output = r.ok();
    assert!(output.contains("[approval] Allow the MCP tool `fake.delete_all` with {}?"), "{output}");
    let denied = &requests[1]["messages"][2]["content"][0];
    assert_eq!(denied["is_error"], true);
    assert!(denied["content"].as_str().unwrap().starts_with("ApprovalDenied: rejected by the human"), "{denied}");
    assert_eq!(requests[2]["messages"][4]["content"][0]["content"], "3");
    // unless the server is trusted
    let r = run_with(&agent("mcp(:fake)", ", approve: false"), replies(), &[]);
    let requests = r.requests.clone();
    assert!(!r.ok().contains("[approval]"));
    assert_eq!(requests[1]["messages"][2]["content"][0]["content"], "deleted");
}

#[test]
fn direct_calls_and_capabilities() {
    let url = grenat_mcp::fake::serve_http();
    let out = run(&format!(
        "mcp :fake, url: \"{url}\"\np Mcp.tools(:fake)\np Mcp.call(:fake, \"echo\", {{text: \"hey\"}})\n"
    ));
    assert_eq!(out, "[\"echo\", \"add\", \"delete_all\"]\n~\"hey\"\n");
    let e = run_err(&format!("mcp :fake, url: \"{url}\"\nMcp.call(:fake, \"nope\")\n"), Vec::new());
    assert_eq!(e.ty, "McpError");
    let src = format!(
        "mcp :fake, url: \"{url}\"\ndef ok uses mcp(\"fake\") = Mcp.tools(:fake).size\ndef other uses mcp(\"linear\") = Mcp.tools(:fake).size\np ok\nother\n"
    );
    let r = run_with(&src, Vec::new(), &[]);
    assert_eq!(r.output, "3\n");
    assert_eq!(r.err().message, "the MCP server `fake` is not allowed by `other` (uses mcp(\"linear\"))");
    let e = run_err("Mcp.tools(:nowhere)\n", Vec::new());
    assert_eq!(e.message, "MCP server `:nowhere` is not declared: `mcp :nowhere, url: \"…\"`");
    let e = run_err("mcp :x\n", Vec::new());
    assert_eq!(e.message, "MCP server `x`: give either `url:` or `command:`");
}

#[test]
fn tests_mock_the_servers() {
    let src = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
mcp :linear, url: \"https://mcp.linear.app/mcp\"
agent Pm
  model :fast
  tools mcp(:linear)
  on Plan -> String
    run \"plan\"
  end
end
test \"mocked\" do
  mock_mcp :linear, tools: {\"create_issue\" => \"created L-1\"}
  mock :fast, replies: [call(:linear__create_issue, title: \"SSO\"), \"L-1 created\"]
  assert_equal \"L-1 created\", spawn(Pm).ask(Plan()).trust!
  assert_equal \"created L-1\", Mcp.call(:linear, \"create_issue\", {title: \"x\"}).trust!
end
test \"not mocked\" do
  Mcp.tools(:linear)
end
";
    let parsed = grenat_parser::parse(src);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    let outcomes = run_tests(&parsed.program, options).unwrap();
    assert!(outcomes[0].error.is_none(), "{:?}", outcomes[0].error);
    assert_eq!(
        outcomes[1].error.as_ref().unwrap().message,
        "no MCP servers in tests: `:linear` is not stubbed with `mock_mcp`"
    );
}
