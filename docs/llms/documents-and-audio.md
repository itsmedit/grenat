# Grenat — documents, images, audio and batches (`Pdf`, `Image`, `Audio`, `transcribe`, `batch_map`)

Read this when a model must read a PDF, an image or a recording, when a web page is turned into text, or when many prompt calls can wait for a batch. It assumes the core reference, `llms.txt`.

```ruby
struct Invoice
  number: String
  total: Float
end

## Extracts an invoice.
prompt extract(doc: Attachment) -> ~Invoice using :fast
  user "Extract this invoice.", doc  # an Attachment: Pdf.read(path), Image.read(path), Pdf.url(url), Image.url(url)
end

struct Minutes
  summary: String  ## three sentences at most
  decisions: Array(String)
end

prompt minutes_of(transcript: String) -> ~Minutes using :fast
  user transcript
end

def minutes(path: String) -> Minutes uses llm, fs.read
  transcript = transcribe(:whisper, Audio.read(path))  # a model with kind: transcription
  minutes_of(transcript).check { |m| m.summary.size > 0 }?
end

def totals(dir: String) -> Array(Float) uses llm, fs.read
  # the batch API: half price, answers within 24 h
  Dir.list(dir).batch_map { |name| extract(Pdf.read("#{dir}/#{name}")).check { |i| i.total > 0 }?.total }
end

def page_text(url: String) -> ~String uses net
  Html.text(Http.get(url).body)  # the text of an HTML page, still untrusted
end
```

- `user` takes attachments after the text: `user "Extract this invoice.", Pdf.read(path)` (also `Image.read`, `Pdf.url`, `Image.url`, `Audio.read`, `Audio.url`).
- `transcribe(:whisper, Audio.read("call.mp3"))` gives the text of a recording; it also takes `language:` and `keywords:`.
- `xs.batch_map { |x| prompt_call(x) }`: batch API, half price, answers within 24 h.
- `Html.text(html)` is a page's text; `Html.escape(s)` escapes a value for a page.

## Tests

`mock_transcribe :whisper, text: "…"` stands for the transcription model.

```ruby
test "a recording becomes minutes" do
  File.write("weekly.mp3", "ID3")
  mock_transcribe :whisper, text: "Ada: we ship on Friday."
  mock :fast, replies: [{summary: "The release ships on Friday.", decisions: ["Ship on Friday"]}]
  assert_equal ["Ship on Friday"], minutes("weekly.mp3").decisions
end

test "invoices in a batch" do
  File.write("inbox/a.pdf", "%PDF-1.7")
  mock :fast, replies: [{number: "F-1", total: 12.5}]
  assert_equal [12.5], totals("inbox")
end
```
