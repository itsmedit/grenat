# Grenat — MCP servers and `expose`

Read this when an agent uses the tools of an MCP server (Linear, Notion, GitHub…), or when a program serves its own tools and agents to others over MCP. It assumes the core reference, `llms.txt` (tools, agents).

```ruby
mcp :linear, url: "https://mcp.linear.app/mcp", headers: {
  "Authorization" => "Bearer #{Credentials.fetch(:linear, :token)}",
}

agent Pm
  model :smart
  tools mcp(:linear)  # the server's tools, named linear__<tool>
  max_turns 10
  instructions "Turn product specs into Linear issues."

  on Plan(spec: String) -> ~String
    run "Create the issues for: #{spec}"
  end
end

def plan(spec: String) -> ~String uses llm, mcp
  spawn(Pm).ask(Plan(spec:))
end

## Reads a documentation page.
tool read_doc(path: String) -> String uses fs.read("./docs")
  File.read("./docs/#{path}")
end

expose "/mcp", tools: [:read_doc], agents: [Pm], token: Credentials.fetch(:api, :token)
```

- `mcp :name, url: "…", headers: {…}` declares a server; `tools mcp(:linear)` gives its tools to an agent; the effect is `mcp` (or `mcp("linear")`). MCP results are untrusted.
- `expose "/mcp", tools: [:read_doc], agents: [Support], token: Credentials.fetch(:api, :token)` serves tools and agents over MCP next to `grenat serve`'s routes (`public: true` instead of `token:` serves without a token). Each handler of an exposed agent is a tool too (`pm_plan`); an agent's message arrives untrusted.

## Tests

`mock_mcp :linear, tools: {"create_issue" => "L-1"}` stands for a server; a mocked model calls its tools as `call(:linear__create_issue, …)`. `request :post, "/mcp/<tool>", json: {…}, headers: {…}` calls an exposed tool (its `"body"` is `{"result": …}`; `"status"` 401 without the token).

```ruby
test "a spec becomes issues" do
  mock_mcp :linear, tools: {"create_issue" => "L-1"}
  mock :smart, replies: [call(:linear__create_issue, title: "SSO sign-in"), "Created L-1"]
  assert_equal "Created L-1", plan("SSO").trust!
end

test "an exposed tool answers with its token" do
  File.write("./docs/refunds.md", "Refunds: 30 days.")
  r = request :post, "/mcp/read_doc", json: {path: "refunds.md"}, headers: {"Authorization" => "Bearer test-api-token"}
  assert_equal({"result" => "Refunds: 30 days."}, Json.parse(r["body"]))
  assert_equal 401, request(:post, "/mcp/read_doc", json: {path: "refunds.md"})["status"]
end
```
