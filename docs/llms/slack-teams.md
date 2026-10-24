# Grenat — Slack and Teams (approvals in Slack, `Slack.post`, signed webhooks)

Read this when approvals should be decided from Slack, a program posts to Slack, or it receives Slack or Teams events. It assumes the core reference, `llms.txt` (workflows, jobs, `approve!`).

`approvals_in_slack` posts every question `approve!` asks in a job to a channel, with Approve and Deny buttons that decide it in one click; `on_webhook` checks Slack's and Teams' signatures.

```ruby
approvals_in_slack(
  channel: "#approvals",
  token: Credentials.fetch(:slack, :bot_token),
  signing_secret: Credentials.fetch(:slack, :signing_secret),
  allowed: ["U024BE7LH"],
)

workflow refund(order: Int) uses human, net("slack.com")
  step(:ask) { approve! "Refund order #{order}?" }  # posted to #approvals with buttons
  step(:tell) { Slack.post("#support", "Order #{order} refunded") }
end

on_webhook "/slack/events", signature: :slack, secret: Credentials.fetch(:slack, :signing_secret) do |req|
  event = req.json.check { |e| e["type"] == "event_callback" }?
  enqueue(:refund, 42) if event["event"]["type"] == "app_mention"
  nil
end
```

- `approvals_in_slack` is declared once: `channel:`, `token:` (the bot token), `signing_secret:`, optional `allowed:` (Slack user IDs who may decide) and `path:` (the Interactivity Request URL, `/slack/interactivity` by default; no webhook or route may claim it); it needs `net("slack.com")`.
- A click is decided as `Approvals.approve`/`deny` (recorded as decided by `slack:U…`) and the message updated; a click on a decided approval changes nothing.
- `Slack.post(channel, text)` posts (`token:` or the credential `slack.bot_token`), needs `net("slack.com")`, returns the message's `ts` (untrusted) and refuses untrusted text (check it first).
- `signature: :slack` checks `X-Slack-Signature` (five minutes, replays refused) and answers `url_verification` itself; `signature: :teams` takes the outgoing webhook's base64 security token.

## Tests

`deliver_slack_click(id, action: :approve, user: "U024BE7LH")` clicks approval `id`'s button in Slack (only inside a `test` block); it returns `{"status" => 200, "outcome" => "approved"}`, or `denied`, `already_decided`, `not_allowed`, `unknown`.

```ruby
test "approved in Slack" do
  mock_http "POST https://slack.com/api/*", json: {ok: true, ts: "1700000000.000100"}
  enqueue(:refund, 42)
  Jobs.perform
  r = deliver_slack_click(Approvals.pending.first["id"], action: :approve, user: "U024BE7LH")
  assert_equal "approved", r["outcome"]
end

test "a user not allowed cannot decide" do
  mock_http "POST https://slack.com/api/*", json: {ok: true, ts: "1700000000.000100"}
  enqueue(:refund, 43)
  Jobs.perform
  r = deliver_slack_click(Approvals.pending.first["id"], action: :approve, user: "U999")
  assert_equal "not_allowed", r["outcome"]
end
```
