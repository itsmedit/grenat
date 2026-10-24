# Grenat — agents in depth (`tell`, pools, `race`, budgets, tool timeouts)

Read this when several agents work together, when messages are sent without waiting, or when spending and tool calls must be bounded. It assumes the core reference, `llms.txt` (tools, agents, `run`).

```ruby
struct Draft
  text: String
end

agent Writer
  model :fast
  max_turns 4
  tool_timeout 20  # seconds each tool call may take
  instructions "Write one short paragraph."

  @notes: Array(String) = []

  on Write(topic: String) -> ~Draft
    run "Write about #{topic}. Notes: #{@notes.join(", ")}"
  end

  on Note(text: String) -> Bool
    @notes << text
    true
  end
end

def drafts(topics: Array(String)) -> Array(String) uses llm
  pool = spawn_pool(Writer, size: 4)  # each message goes to the least busy agent
  within budget(usd: 1.00) do
    topics.parallel_map(limit: 4) { |t| pool.ask(Write(topic: t)).check { |d| d.text.size > 0 }?.text }
  rescue BudgetExceeded => e
    []
  end
end

def fastest(topic: String) -> ~Draft uses llm
  a = spawn(Writer)
  b = spawn(Writer)
  a.tell(Note(text: "be brief"))  # sends without waiting for the answer
  race do                         # the first branch to succeed wins, the others are cancelled
    a.ask(Write(topic:))
    b.ask(Write(topic:))
  end
end
```

- `a.ask(Msg(...))` waits for the answer; `a.tell(Msg(...))` sends without waiting.
- `spawn_pool(Writer, size: 4)` is a load-balanced pool: each message goes to the least busy agent.
- `xs.parallel_map(limit: 5) { |x| … }` runs the block concurrently, at most `limit` at a time; `race do … end` runs each statement as a branch, the first to succeed wins, the others are cancelled (all failing: the first error is raised).
- `within budget(usd: 1.00) do … rescue BudgetExceeded => e … end` caps spending (an inner budget never exceeds its enclosing one); an agent's own `budget usd: 0.50, time: 2.min` caps each `run`.
- `tool_timeout 20` in an agent bounds each tool call (seconds).
- Mocked models cost nothing: a test proves a limit with `max_turns` (more tool-call replies than turns), not with a dollar budget.

## Tests

```ruby
test "drafts from a pool" do
  mock :fast, replies: [{text: "One."}, {text: "Two."}]
  assert_equal 2, drafts(["a", "b"]).size
end

test "the first draft wins" do
  mock :fast, replies: [{text: "Fast."}, {text: "Slow."}]
  assert fastest("rust").trust!.text.end_with?(".")
end
```
