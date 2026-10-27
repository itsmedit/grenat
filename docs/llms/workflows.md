# Grenat — workflows, jobs and schedules in depth (`return`, journaled values, `enqueue`, `every`)

Read this when a workflow returns early, checks a model's answer, is queued with keyword arguments or runs on a schedule. It assumes the core reference, `llms.txt` (workflows, steps, `enqueue`).

```ruby
struct Digest
  monday: String
  text: String  ## the week's summary
end

prompt summarize_week(monday: String, repos: Array(String)) -> ~String using :fast
  user "Summarize the week of #{monday} for #{repos.join(", ")}."
end

workflow weekly(monday: String, repos: Array(String) = ["rust-lang/rust"]) -> Digest uses llm, net("hooks.acme.io")
  return Digest(monday:, text: "nothing to watch") if repos.empty?
  text = step(:summary) do
    summarize_week(monday, repos).check { |t| t.size.between?(1, 2000) }?
  end
  step(:post) { Http.post("https://hooks.acme.io/digest", json: {monday:, text:}).status }
  Digest(monday:, text:)
end

def queue_week(monday: String) uses db
  enqueue(:weekly, monday:)
end

every cron: "0 8 * * MON" do  # UTC
  queue_week(Time.today)
end
```

## Runs, steps and what is journaled

- A run is the workflow and its arguments, matched to the parameters: `weekly("2026-09-28")`, `weekly(monday: "2026-09-28")` and a job queued either way are one run, whatever the order of the keywords.
- `return` works in a workflow, and in a step's block: the run ends and its value is journaled as the result; called again with the same arguments, the run returns it at once, without running anything.
- A step's value and the result are journaled: they must be data — `nil`, numbers, strings, symbols, arrays, hashes, structs, enums, errors (else `TypeError: step :x must return data`). Never a `Secret` (fetch it where it is used), an agent (`spawn` it outside the steps) or a class instance. `Db.connect` needs no step.
- Taint is journaled too: an untrusted step value is untrusted when replayed. Check a model's answer inside its step (`summarize_week(…).check { … }?` above): a failing check fails the step, nothing is journaled, and the next run asks the model again. Checked after the step (`step(:s) { ask(x) }.check { … }?`), the bad answer is journaled and every replay fails the same way.
- A workflow whose step raised is not completed: the next call with the same arguments replays the finished steps and runs the failed one again.

## Jobs

`enqueue(:weekly, "2026-09-28")`, `enqueue(:weekly, monday: "2026-09-28", in: 1.hour)`: the worker calls the function with these arguments, positional and keyword, as a call would; `grenat check` holds them to the function's parameters (E0200) and the name to a function of the program (E0100). Arguments are data and trusted (E0412). In a test, `Jobs.enqueued` lists `[name, args]`, the keywords last as a hash (`[[:weekly, [{monday: "2026-09-28"}]]]`), `Jobs.perform` runs them, `Jobs.failed` gives `[name, error]`. A failed job is retried twice (1 then 2 minutes later); a workflow job resumes from its journal.

## Schedules and other top-level blocks

`every cron: "0 8 * * MON" do … end` (or `every 1.hour do … end`) runs its block under `grenat serve`, in UTC. Like routes, webhooks, `database` and `on_email`, it is a top-level declaration: it declares no `uses`, and what it calls brings its own (`queue_week` declares `db`). Call a function from it for quick work; `enqueue` a workflow, so that it is retried, resumable and may wait for a human (`approve!` waits only in a job). A test cannot fire a schedule: test the function it calls.

## Tests

To simulate a crash between steps, make the later step fail (`mock_mail(raise: "SMTP down")`), assert the error, undo it (`mock_mail(raise: nil)`), run the workflow again with the same arguments: finished steps replay. A `mock` with no reply left fails the test: it proves the model was not called again.

```ruby
test "a week is summarized once, however it is called" do
  mock_http "POST https://hooks.acme.io/digest", json: {}
  mock :fast, replies: ["Two releases."]
  assert_equal "Two releases.", weekly("2026-09-28").text
  queue_week("2026-09-28")
  assert_equal [[:weekly, [{monday: "2026-09-28"}]]], Jobs.enqueued
  Jobs.perform  # the same run: replayed, the model not called again
  assert_equal [], Jobs.failed
  assert_equal "nothing to watch", weekly("2026-10-05", []).text
end

test "a bad answer is asked again" do
  mock_http "POST https://hooks.acme.io/digest", json: {}
  mock :fast, replies: ["", "One release."]
  assert_raises CheckError { weekly("2026-10-12") }
  assert_equal "One release.", weekly(monday: "2026-10-12").text
end
```
