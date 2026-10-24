# Grenat — streaming answers and conversations (`stream`, `Conversation`)

Read this when an answer must reach a browser as the model writes it (Server-Sent Events), or when a chat keeps its turns between messages or sessions. It assumes the core reference, `llms.txt`.

```ruby
## Answers a customer.
prompt reply(question: String) -> ~String using :fast
  system "Answer in three sentences at most."
  user question
end

get "/chat" do |req|
  stream do |out|
    answer = reply(req.params["q"]) { |chunk| out << chunk }  # each piece, sent at once
    out.event("done", {"chars" => answer.size})
  end
end

def chat(line: String) -> ~String uses llm, fs.read, fs.write
  convo = Conversation.load("chat.json", model: :fast, system: "You are a helpful assistant.")
  answer = convo.say(line)  # untrusted, as any model answer
  convo.save("chat.json")
  answer
end
```

- `stream do |out| … end` answers as Server-Sent Events: `out << data` sends an event (a string as it is, any other value as JSON), `out.event("name", data)` a named one.
- A prompt call, an agent's `ask` and a conversation's `say` take a block that receives each piece (`~String`, untrusted) and still return the whole answer; only an answer that is a `String` streams.
- `Conversation.load(path, model: :fast, system: "…")` (a missing file starts empty; `keep: 10` keeps that many turns, older ones summed up) → `say(text)` (untrusted answer), `save(path)`; `Conversation.new(model: :fast)` keeps it in memory; `history` lists the turns.

## Tests

`request` collects a streamed response: `events` lists `{"event" => name, "data" => data}` (`message` naming the unnamed).

```ruby
test "the answer arrives as events" do
  mock :fast, replies: ["Hello, Ada"]
  r = request(:get, "/chat?q=hi")
  assert_equal "Hello, Ada", r["events"].first["data"]
  assert_equal "done", r["events"].last["event"]
end

test "the conversation remembers" do
  mock :fast, replies: ["Hi Ada!", "You are Ada."]
  chat("I am Ada")
  assert_equal "You are Ada.", chat("Who am I?").trust!
end
```
