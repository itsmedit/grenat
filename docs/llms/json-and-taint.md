# Grenat — JSON and untrusted values in depth (`res.json`, `Json.parse`, `dig`, structs)

Read this when a program reads fields of a JSON body (an HTTP response, a webhook, `Json.parse`) or builds structs from them. It assumes the core reference, `llms.txt` (taint, `check`).

```ruby
struct Release
  tag: String
  stars: Int
end

def latest(repo: String) -> Release uses net("api.github.com")
  res = Http.get("https://api.github.com/repos/#{repo}/releases/latest")
  tag = res.json["tag_name"].check { |t| t.is_a?(String) && t.size.between?(1, 40) }?
  stars = res.json.dig("repo", "stars").check { |n| n.is_a?(Int) && n >= 0 }?
  Release(tag:, stars:)
end

def assets(repo: String) -> Array(String) uses net("api.github.com")
  res = Http.get("https://api.github.com/repos/#{repo}/releases/latest")
  names = res.json.fetch("assets", []).map { |a| a["name"] }
  names.check { |ns| ns.all? { |n| n.is_a?(String) && !n.include?("/") } }?
end

def unchecked(repo: String) -> Release uses net("api.github.com")
  res = Http.get("https://api.github.com/repos/#{repo}/releases/latest")
  Release(tag: res.json["tag_name"], stars: 0)  # an untrusted Release
end
```

- `res.json`, `req.json` and `Json.parse(text)` are untrusted (`Json.parse` of a trusted text is trusted), and so is everything read from them: `res.json["id"]`, `res.json["items"].map { |h| h["name"] }`. Their type is unknown to the checker: it accepts them where any type is expected, and nothing converts or verifies them at run time — `Release(tag: res.json["id"])` holds an `Int` in a `String` field. A `.check` that tests the type (`t.is_a?(String)`) is what makes a field both trusted and of its type; a block that fails on the wrong type (`i > 0` on a string) raises `TypeError`, not `CheckError`.
- A missing key reads `nil`, and indexing `nil` raises `TypeError`: read nested fields with `json.dig("repo", "stars")` (nil if any level is missing) and defaults with `json.fetch("assets", [])`. An array of hashes is an array: `map`, `select`, `first`… then check the result.
- Taint follows values into what is built from them: a struct, an array, a hash, an interpolated string, a function's parameter and its result. `unchecked(repo).tag` sent to another host is E0412, though the struct is built from a single untrusted field. Declaring a type never cleans: a function declared `-> Release` (or `-> String`) that returns an untrusted value returns it untrusted, and `grenat check` names its type without `~` (`unknown field or method x for Release`): taint is tracked on values, apart from their types.
- Clean at the boundary, once: check each field where it is read, build the struct from checked values (`latest` above), and the struct is trusted.

## Tests

```ruby
test "fields are read, typed and checked" do
  mock_http "GET https://api.github.com/repos/*", json: {tag_name: "v1.2", repo: {stars: 7}, assets: [{name: "a.tar.gz"}, {name: "b.zip"}]}
  r = latest("acme/app")
  assert_equal "v1.2", r.tag
  assert_equal 7, r.stars
  assert_equal ["a.tar.gz", "b.zip"], assets("acme/app")
end

test "a field of the wrong type fails its check" do
  mock_http "GET https://api.github.com/repos/*", json: {tag_name: 12, repo: {stars: 7}}
  assert_raises CheckError { latest("acme/app") }
end

test "a missing level reads nil" do
  mock_http "GET https://api.github.com/repos/*", json: {tag_name: "v1"}
  assert_raises CheckError { latest("acme/app") }
  assert_equal [], assets("acme/app")
end
```
