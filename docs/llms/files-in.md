# Grenat — files in (`on_file`, `deliver_file`)

Read this when a program must process files dropped in a folder (scans, exports, invoices). It assumes the core reference, `llms.txt`.

`grenat serve` watches a folder (inotify on Linux, FSEvents on macOS, and a scan every `every:`) and hands each file matching the pattern to its handler once it has stopped changing for `settle:`, once per content (SHA-256; kept in the `database` across restarts).

```ruby
struct Invoice
  number: String
  total: Float
end

prompt extract(pdf: Attachment) -> ~Invoice using :fast
  user "Extract this invoice.", pdf
end

on_file "inbox/**/*.{pdf,PDF}", settle: 5.s do |path|
  # reading `path` needs no check, but writing to it or running it would
  invoice = extract(Pdf.read(path)).check { |i| i.total > 0 && i.number.size > 0 && i.number.count("^A-Za-z0-9-") == 0 }?
  File.write("filed/#{invoice.number}.json", Json.dump(invoice))  # the number names a file: letters, digits, dashes only
end
```

- `path` is a `String` as the pattern writes it (`inbox/2026/a.pdf`), untrusted: the dropper chose the name, so check it before a command, a request or a written path.
- Patterns: `*`, `?`, `[a-z]`, `{a,b}`, `**`; hidden names (`.x`) are left out.
- Options: `settle:` (2 s), `every:` (10 s), `existing: false` (skip the files there at start).
- Needs `fs.read` on the folder (`fs.read("inbox")`).
- A link leading out of the folder is refused, checked on the file opened: `File.read`/`File.lines`/`Pdf.read`/`Image.read`/`Audio.read` of a path under a watched folder raise `IoError` if it was swapped for a link leading out.
- A raising handler is retried three times, then given up until the file changes.

## Tests

`deliver_file("inbox/a.pdf")` runs every `on_file` handler matching the path (only inside a `test` block); it returns `{"status" => "handled"}` or `"failed"` with `"error"`.

```ruby
test "an invoice is filed" do
  File.write("inbox/a.pdf", "%PDF-1.7")
  mock :fast, replies: [{number: "F-42", total: 120.0}]
  assert_equal "handled", deliver_file("inbox/a.pdf")["status"]
  assert File.read("filed/F-42.json").include?("F-42")
end
```
