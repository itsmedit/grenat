# html

Grenat's official facet for reading HTML with CSS selectors: the text, the attributes or the HTML of the elements a selector matches, the links of a page as absolute URLs, the rows of a table.

It is a **native facet**: its functions are Rust code (`native/`, a crate depending on `grenat_ext`, built on [scraper](https://crates.io/crates/scraper), the HTML parser of the Servo browser engine), called by Grenat programs as ordinary functions.

## Installing

Native code runs outside Grenat's sandbox, so the application trusts it explicitly, in its `Facetfile`:

```ruby
facet "html", "~> 0.1", native: true
# or, from a checkout of Grenat:
facet "html", path: "../grenat/facets/html", native: true
```

`setter install` then builds the crate (`cargo build --release`: a Rust toolchain is needed) and writes the Grenat declarations of its functions. Without `native: true`, `setter install` refuses, and says how to trust it.

```ruby
require "html"
```

## Functions

| Function | Returns |
|---|---|
| `select(html, selector)` | the text of each element `selector` matches, in document order, blank space collapsed |
| `select_attr(html, selector, attribute)` | the value of `attribute` (`href`, `src`…) of each match that has it |
| `select_html(html, selector)` | the outer HTML of each match, its tags included |
| `links(html, base_url)` | the absolute URLs of the page's links (`<a href>`), each once, in document order; `javascript:` and `mailto:` links left out |
| `all_links(html, base_url)` | the same, `javascript:` and `mailto:` links included |
| `table(html, selector)` | the rows of the first table `selector` matches, each as the text of its cells (`<th>`, `<td>`): `Array(Array(String))` |

Written in Grenat on top of them (`src/lib.grn`):

| Function | Returns |
|---|---|
| `select_first(html, selector)` | the text of the first match, or `nil` |
| `matches?(html, selector)` | whether `selector` matches at least one element |
| `table_records(html, selector)` | the rows of a table after its first, as hashes from each column's name (the first row) to the row's cell (`""` where a row is shorter) |

```ruby
require "html"

PAGE = <<~HTML
  <h1>The café</h1>
  <ul><li class="drink">Tea</li><li class="drink">Coffee <b>-20%</b></li></ul>
  <a href="/about">About</a> <a href="mailto:hi@example.com">Write to us</a>
  <img src="/logo.png" alt="Logo">
  <table id="prices">
    <tr><th>Item</th><th>Price</th></tr>
    <tr><td>Tea</td><td>3</td></tr>
  </table>
HTML

def main
  select(PAGE, "li.drink")                         # ["Tea", "Coffee -20%"]
  select_attr(PAGE, "img", "src")                  # ["/logo.png"]
  select_html(PAGE, "li b")                        # ["<b>-20%</b>"]
  links(PAGE, "https://cafe.example.com/menu/")    # ["https://cafe.example.com/about"]
  all_links(PAGE, "https://cafe.example.com/")     # ["https://cafe.example.com/about", "mailto:hi@example.com"]
  table(PAGE, "#prices")                           # [["Item", "Price"], ["Tea", "3"]]
  table_records(PAGE, "#prices")                   # [{"Item" => "Tea", "Price" => "3"}]
  select_first(PAGE, "h1")                         # "The café"
end
```

- **Documents and fragments.** `html` may be a whole page or a fragment: it is parsed as a browser parses a page, entities decoded and unclosed tags closed.
- **Links.** Relative URLs are resolved against `base_url` — or against the page's own `<base href>`, as a browser does. An `href` that is not a valid URL is left out.
- **Tables.** The rows of the first table matched: in `<thead>`, `<tbody>`, `<tfoot>` or directly in it, not those of a table nested in one of its cells (whose text belongs to that cell). No table matched, no rows.

## Errors

An invalid selector, a base URL that is not an absolute URL, a table selector matching another element than a `<table>`, an empty attribute name: each raises an `HtmlError` saying what is wrong and naming it, never a crash.

```ruby
begin
  select(page, "li[")
rescue HtmlError => e
  puts e.message   # invalid CSS selector `li[`: it ends too early
end
```

## Trust

Every function is **pure**: no effect to declare, and its result depends on its arguments only — so it is exactly as trusted as the HTML it is given. What a program wrote itself is trusted; a page fetched with `Http` is untrusted (`~String`), and so is everything read from it: a link found in it cannot be fetched, nor its text put in a command or a page, before it is checked.

```ruby
def follow(url: String) -> Int uses net
  page = Http.get(url).body                    # untrusted: a web page can say anything
  next_url = links(page, url).first            # untrusted too
  Http.get(next_url).status                    # error[E0412]: an untrusted value reaches `Http.get`
end

def follow_checked(url: String) -> Int uses net
  page = Http.get(url).body
  next_url = links(page, url).first.check { |u| u.start_with?("https://shop.example.com/") }?
  Http.get(next_url).status                    # checked: allowed
end
```

The facet reads no file and opens no connection.

## Developing

```sh
cargo test --manifest-path native/Cargo.toml   # the Rust unit tests
setter install && grenat test                   # the Grenat tests (tests/*.grn)
```

The crate is not a member of Grenat's workspace (its own `[workspace]` and `Cargo.lock`); Grenat's test suite builds it, installs it into an application and runs both (`crates/grenat_cli/tests/facet_html.rs`).
