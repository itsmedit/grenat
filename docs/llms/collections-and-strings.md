# Grenat — strings, arrays and hashes (methods, set operators, slices, formats)

Read this when a task manipulates text or collections beyond `map`/`select`/`join`, to know which Ruby methods exist and how slices and formats behave. It assumes the core reference, `llms.txt` (no regular expressions, no `each_char`, no destructuring block parameters).

```ruby
## The first words of a title, for a listing.
def teaser(title: String) -> String
  words = title.split(" ")
  words.size > 3 ? "#{words.take(3).join(" ")}…" : title
end

## Order lines grouped by four, with a total each.
def totals(amounts: Array(Float)) -> Array(Float)
  amounts.each_slice(4).map { |group| group.sum.round(2) }
end

## A product's tags, from nested lists and a code like "sku-0042".
def tags(nested: Array(Array(String)), code: String) -> Array(String)
  number = code.delete_prefix("sku-").to_i
  (nested.flatten | ["n#{"%05d" % number}"]).sort
end

def letters(word: String) -> Hash(String, Int)
  word.downcase.split("").tally
end
```

- Ruby's usual methods: `map`, `select`, `each_with_index`, `sort_by`, `take`, `join`, `split`, `include?`, `upcase`, `strip`, `to_i`, `to_f`, `to_s`, `size`, `first`, `any?`, `all?`, `partition`, `group_by`, `sum`, `flatten` / `flatten(depth)` (negative: all levels), `each_slice`, `each_cons`, `each_with_object`, `filter_map`, `tally`, `zip`, `min_by`, `minmax`, `uniq`, `sort { |a, b| b <=> a }`, `reduce(:+)`, `inject(0, :*)`, `inject(0) { … }`, `dig`, `transform_values`, `transform_keys`, `merge(h) { |k, a, b| … }`, `fetch(k, default)`, `delete_prefix`, `delete_suffix`, `start_with?`, `center`, `ljust`, `tr`, `squeeze`, `count("a-z", "^aeiou")` (several sets intersect), `gsub`/`sub` with strings (no patterns), `then`…
- Set operators `a & b` / `a | b` / `a - b` (order of `a`; `&` and `|` drop duplicates; both arrays of the same items) and `intersection`, `union`, `difference`, `intersect?`.
- Slices count characters, as in Ruby: `s[0, 4]`, `s[1..]`, `s[1..-2]`, `xs[2, 3]`, `xs[1...-1]` are `String?` / `Array(T)?` (`nil` when the start is past the end); they are read only (`xs[i] = v` assigns an item, `xs[0, 2] = v` and `s[0] = "x"` are errors: build a new value).
- `format("%.2f", x)`, `"%05d" % n`, `"%.3d" % n`, `"%s-%s" % [a, b]` format numbers as Ruby does (`%x` of a negative is `-ff`, not `..f01`); `x.round(2)` rounds.

## Tests

```ruby
test "strings and collections" do
  assert_equal "Rust is fast…", teaser("Rust is fast and safe")
  assert_equal [10.0, 2.5], totals([1.0, 2.0, 3.0, 4.0, 2.5])
  assert_equal ["a", "b", "n00042"], tags([["b"], ["a", "b"]], "sku-0042")
  assert_equal 2, letters("Noon")["o"]
  assert_equal [2, 3], [1, 2, 3] & [3, 2, 2]
  assert_equal [1], [1, 2, 3] - [2, 3]
  assert_equal "-ff", "%x" % -255
  assert_equal 7, "hello world".count("a-z", "^aeiou")
  assert_equal "ell", "hello"[1..-2]
  assert_equal nil, "abc"[5, 1]
end
```
