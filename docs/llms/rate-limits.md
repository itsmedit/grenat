# Grenat — rate limits (`limit`, `Retry-After`, `mock_http times:`)

Read this when an API documents a rate limit, or answers `429 Too Many Requests`. It assumes the core reference, `llms.txt`.

```ruby
limit net("api.hubspot.com"), 10.per_second
limit net("api.anthropic.com"), 50.per_minute  # model calls, by their provider's host

## A contact's name in the CRM.
def contact(id: Int) -> String uses net("api.hubspot.com")
  res = Http.get("https://api.hubspot.com/crm/v3/objects/contacts/#{id}")
  res.json["name"].check { |n| n.size < 200 }?
end
```

- `limit net("host"), rate` with `10.per_second`, `per_minute`, `per_hour`, `per_day`, `100.per(15.minutes)`, and/or `concurrent: 4`; once per host, as a statement at the top level of the program or of a `test` block (never in a function, block or loop: E0500).
- Every call to the host from any task or job waits its turn (never fails): `Http`, `Slack.post`, sending mail, and model calls by their provider's host.
- `Http`: a `429` with `Retry-After` is waited out and sent again (3 times at most) for GET/HEAD/PUT/DELETE, for POST/PATCH only with `retry: true` (`retry:` is a `Bool`); the host that answered (after redirects) is paused until then.

## Tests

`mock_http … times: n` (a positive `Int`) answers the next n requests only, then gives way to the stubs declared before it (the latest matching stub answers first): a `429` then a `200` is the `200` stub, then the `429` one with `times: 1`. `retry: false` turns the retry off for any method.

```ruby
test "a 429 is waited out" do
  mock_http "GET https://api.hubspot.com/*", json: {name: "Ada"}
  mock_http "GET https://api.hubspot.com/*", status: 429, headers: {"Retry-After" => "0"}, times: 1
  assert_equal "Ada", contact(7)
  assert_equal 2, Http.requests.size
end
```
