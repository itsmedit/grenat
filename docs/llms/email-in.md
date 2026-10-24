# Grenat — email in (`on_email`, `deliver_email`)

Read this when a program must react to incoming email (a support inbox, invoices sent by mail). It assumes the core reference, `llms.txt`.

`grenat serve` reads a mailbox (Gmail, Microsoft 365, any IMAP server) and hands each new message to its handler, then marks it seen (or moves it with `move_to:`); a handler that raises leaves it unseen, tried three times, then flagged.

```ruby
struct Triage
  urgent: Bool
  summary: String
end

prompt triage(subject: String, body: String, documents: Array(Attachment)) -> ~Triage using :fast
  user subject, body
  documents.each { |d| user d }  # attachments go to the model
end

def follow_up(summary: String)
  puts "urgent: #{summary}"
end

on_email Credentials.fetch(:support, :imap_url), every: 1.minute, move_to: "Done" do |email|
  documents = email.attachments.select { |a| a.kind != "file" }  # PDFs, images, audio
  t = triage(email.subject, email.text, documents).check { |t| t.summary.size < 300 }?
  enqueue(:follow_up, t.summary) if t.urgent
end
```

- The URL is `imaps://user%40acme.io:APP-PASSWORD@imap.gmail.com/INBOX` (folder in the path), a `Credentials` secret; `token: :fn` names a function returning an OAuth access token instead (Microsoft 365 requires one).
- `email` is an `IncomingEmail`: `from`, `from_name`, `to`, `cc` (`Array(String)`), `reply_to`, `subject`, `date` (`Float?`), `message_id`, `text`, `html`, `attachments` (`Array(Attachment)`: `kind` `document`/`image`/`audio`/`file`, `media_type`, `name`) — every field untrusted, the sender's address included.
- Reading the mailbox needs `net` (`net("imap.gmail.com")` for a literal URL).
- `every:` is positive, `move_to:` names an existing folder (a refused move flags the message), and `token:` a function of no argument giving a `String` or a `Secret`.
- A message nesting over 100 messages, holding over 1000 parts, or forwarding an encoded message is flagged unparsed.

## Tests

`deliver_email(from:, subject:, text:, attachments: [...])` runs the `on_email` handler (only inside a `test` block); it returns `{"status" => "seen"}`, `"moved"` with `"folder"`, or `"failed"` with `"error"`.

```ruby
test "an urgent email is followed up" do
  mock :fast, replies: [{urgent: true, summary: "Refund asked twice"}]
  r = deliver_email(from: "ada@example.com", subject: "Refund", text: "Second time I ask.")
  assert_equal "moved", r["status"]
  assert_equal "Done", r["folder"]
end
```

A PDF goes in as `attachments: [Pdf.read("invoice.pdf")]`.
