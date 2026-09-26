//! Tools and agents served to other programs: MCP messages, plain JSON
//! calls, tokens, and messages that arrive untrusted.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, run_tests};

fn results(src: &str) -> Vec<(String, Option<String>)> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    run_tests(&parsed.program, options)
        .unwrap()
        .into_iter()
        .map(|o| (o.name, o.error.map(|e| format!("{}: {}", e.ty, e.message))))
        .collect()
}

fn all_pass(src: &str) {
    let results = results(src);
    assert!(!results.is_empty());
    for (name, error) in results {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
}

const APP: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"

## Finds a ticket by its number.
tool find_ticket(id: Int) -> String uses db.read
  \"Ticket #{id}\"
end

## Closes a ticket.
tool close_ticket(id: Int) -> Int uses db.write
  if id > 100
    raise ArgumentError, \"no ticket #{id}\"
  end
  id
end

agent Triage
  model :fast

  ## Classifies a ticket.
  on Classify(subject: String) -> ~String
    run \"Classify: #{subject}\"
  end

  on Page(text: String) -> String
    html(text)
    \"shown\"
  end
end

expose \"/mcp\", tools: [:find_ticket, :close_ticket], agents: [Triage], token: \"s3cret\", name: \"support\"

AUTH = {\"Authorization\" => \"Bearer s3cret\"}

def mcp(method: String, params: Hash(String, Any)) -> Hash(String, Any)
  message = {\"jsonrpc\" => \"2.0\", \"id\" => 1, \"method\" => method, \"params\" => params}
  Json.parse(request(:post, \"/mcp\", json: message, headers: AUTH)[\"body\"]).trust!
end

def call(name: String, arguments: Hash(String, Any)) -> Hash(String, Any)
  mcp(\"tools/call\", {\"name\" => name, \"arguments\" => arguments})[\"result\"]
end
";

#[test]
fn an_mcp_client_lists_and_calls_the_exposed_tools() {
    all_pass(&format!(
        "{APP}
test \"initialize\" do
  info = mcp(\"initialize\", {{}})[\"result\"]
  assert_equal \"support\", info[\"serverInfo\"][\"name\"]
end

test \"tools/list\" do
  tools = mcp(\"tools/list\", {{}})[\"result\"][\"tools\"]
  assert_equal [\"find_ticket\", \"close_ticket\", \"triage_classify\", \"triage_page\"], tools.map {{ |t| t[\"name\"] }}
  assert_equal \"Finds a ticket by its number.\", tools[0][\"description\"]
  assert_equal [\"id\"], tools[0][\"inputSchema\"][\"required\"]
  assert_equal true, tools[0][\"annotations\"][\"readOnlyHint\"]
  assert_equal false, tools[1][\"annotations\"][\"readOnlyHint\"]
  assert_equal \"Classifies a ticket.\", tools[2][\"description\"]
end

test \"a tool\" do
  result = call(\"find_ticket\", {{\"id\" => 42}})
  assert_equal \"Ticket 42\", result[\"content\"][0][\"text\"]
  assert_equal false, result[\"isError\"]
end

test \"what a tool raises is a tool error\" do
  result = call(\"close_ticket\", {{\"id\" => 500}})
  assert_equal true, result[\"isError\"]
  assert_equal \"ArgumentError: no ticket 500\", result[\"content\"][0][\"text\"]
  assert call(\"find_ticket\", {{\"id\" => \"x\"}})[\"isError\"]
end

test \"an unknown tool is a protocol error\" do
  reply = mcp(\"tools/call\", {{\"name\" => \"drop_tables\", \"arguments\" => {{}}}})
  assert_equal(-32602, reply[\"error\"][\"code\"])
end

test \"a notification gets no response\" do
  r = request :post, \"/mcp\", json: {{\"jsonrpc\" => \"2.0\", \"method\" => \"notifications/initialized\"}}, headers: AUTH
  assert_equal 202, r[\"status\"]
end
"
    ));
}

#[test]
fn an_agent_answers_through_its_handlers() {
    all_pass(&format!(
        "{APP}
test \"a handler is a tool\" do
  mock :fast, replies: [\"billing\"]
  result = call(\"triage_classify\", {{\"subject\" => \"Refund please\"}})
  assert_equal \"billing\", result[\"content\"][0][\"text\"]
end

test \"its message arrives untrusted\" do
  result = call(\"triage_page\", {{\"text\" => \"<script>\"}})
  assert result[\"isError\"]
  assert result[\"content\"][0][\"text\"].start_with?(\"TaintError\")
end
"
    ));
}

#[test]
fn plain_json_endpoints_and_the_catalogue() {
    all_pass(&format!(
        "{APP}
test \"a call\" do
  r = request :post, \"/mcp/find_ticket\", json: {{\"id\" => 7}}, headers: AUTH
  assert_equal 200, r[\"status\"]
  assert_equal \"Ticket 7\", Json.parse(r[\"body\"]).trust![\"result\"]
end

test \"its failures\" do
  assert_equal 422, request(:post, \"/mcp/close_ticket\", json: {{\"id\" => 500}}, headers: AUTH)[\"status\"]
  assert_equal 404, request(:post, \"/mcp/nothing\", json: {{}}, headers: AUTH)[\"status\"]
  assert_equal 400, request(:post, \"/mcp/find_ticket\", body: \"{{oops\", headers: AUTH)[\"status\"]
  assert_equal 405, request(:get, \"/mcp/find_ticket\", headers: AUTH)[\"status\"]
end

test \"the catalogue\" do
  r = request :get, \"/mcp\", headers: AUTH
  catalogue = Json.parse(r[\"body\"]).trust!
  assert_equal \"support\", catalogue[\"name\"]
  assert_equal 4, catalogue[\"tools\"].size
  events = request :get, \"/mcp\", headers: {{\"Authorization\" => \"Bearer s3cret\", \"Accept\" => \"text/event-stream\"}}
  assert_equal 405, events[\"status\"]
end
"
    ));
}

#[test]
fn a_token_is_required() {
    all_pass(&format!(
        "{APP}
test \"without a token\" do
  r = request :post, \"/mcp/find_ticket\", json: {{\"id\" => 7}}
  assert_equal 401, r[\"status\"]
  assert_equal \"Bearer\", r[\"headers\"][\"WWW-Authenticate\"]
  wrong = request :post, \"/mcp\", json: {{}}, headers: {{\"Authorization\" => \"Bearer guess\"}}
  assert_equal 401, wrong[\"status\"]
end
"
    ));
}

#[test]
fn a_public_exposure_needs_no_token() {
    all_pass(
        "tool ping -> String
  \"pong\"
end
expose \"/tools\", tools: [:ping], public: true
test \"public\" do
  r = request :post, \"/tools/ping\"
  assert_equal \"{\\\"result\\\":\\\"pong\\\"}\", r[\"body\"]
end
",
    );
}

#[test]
fn declarations_are_checked() {
    let error = |src: &str| {
        let e = run_err(src, Vec::new());
        format!("{}: {}", e.ty, e.message)
    };
    let tool = "tool ping -> String\n  \"pong\"\nend\ndef plain = 1\n";
    assert_eq!(
        error(&format!("{tool}expose \"/mcp\", tools: [:ping]")),
        "ArgumentError: `expose \"/mcp\"` serves anyone: give it a `token:`, or say `public: true`"
    );
    assert_eq!(
        error(&format!("{tool}expose \"/mcp\", tools: [:plain], public: true")),
        "NameError: `plain` is not a tool (declared with `tool`)"
    );
    assert_eq!(
        error(&format!("{tool}expose \"/mcp\", tools: [:ping, :ping], public: true")),
        "ArgumentError: `expose` offers two tools named `ping`"
    );
    assert_eq!(
        error(&format!("{tool}expose \"/\", tools: [:ping], public: true")),
        "ArgumentError: `expose` takes a path such as \"/mcp\", got \"\""
    );
    assert_eq!(
        error(&format!(
            "{tool}expose \"/a\", tools: [:ping], public: true\nexpose \"/a/\", tools: [:ping], public: true"
        )),
        "ArgumentError: `/a` is exposed twice"
    );
}
