//! A message arriving, in tests: `deliver_email(from: "ada@acme.com",
//! subject: "Refund", text: "…", attachments: [Pdf.read("invoice.pdf")])`
//! hands the `on_email` handler the message its mailbox would give it —
//! every field untrusted — without opening a connection, and says what
//! became of it: `{"status" => "seen"}` (or `"moved"`, with `"folder"`),
//! or `{"status" => "failed", "error" => "ArgumentError: …"}` when the
//! handler raised (served, the message would be tried again).
//!
//! With several `on_email` declarations, `folder:` says whose mailbox
//! (the folder of its URL) receives it.

use grenat_imap::Message;

use crate::builtins::{ATTACHMENT, incoming_email, iso8601, number};
use crate::prelude::*;

const USAGE: &str = "deliver_email(from: \"ada@acme.com\", subject: \"…\", text: \"…\")";

impl<'p> Interp<'p> {
    /// `deliver_email(from:, to:, cc:, subject:, text:, html:, attachments:, …)`.
    pub(crate) fn deliver_email(&mut self, args: &Args<'p>) -> R<'p> {
        self.only_in_tests("deliver_email", USAGE)?;
        if args.named.iter().any(|(_, v)| v.contains_secret()) {
            return raise("SecretError", "a secret is no part of an email: `deliver_email` takes plain values");
        }
        if !args.pos.is_empty() {
            return raise("ArgumentError", format!("`deliver_email` takes named fields: `{USAGE}`"));
        }
        let folder = named_text(args, "folder")?;
        let (block, move_to) = self.recipient(folder.as_deref())?;
        let (message, attachments) = self.message_of(args)?;
        let email = incoming_email(&message, attachments);
        let outcome = match self.call_block(&block, vec![email]) {
            Ok(_) => match move_to {
                Some(folder) => vec![("status", Value::str("moved")), ("folder", Value::str(folder))],
                None => vec![("status", Value::str("seen"))],
            },
            Err(Ctrl::Raise(error)) => {
                let error = format!("{}: {}", error.ty, error.message);
                vec![("status", Value::str("failed")), ("error", Value::str(error))]
            }
            Err(other) => return Err(other),
        };
        let pairs = outcome.into_iter().map(|(k, v)| (Value::str(k), v)).collect();
        Ok(Value::Hash(Arc::new(Mutex::new(pairs))))
    }

    /// The handler a message goes to, and where it moves its messages.
    fn recipient(&self, folder: Option<&str>) -> Result<(Value<'p>, Option<String>), Ctrl<'p>> {
        let mailboxes = self.mailboxes.borrow();
        let matching: Vec<_> = mailboxes
            .iter()
            .filter(|m| folder.is_none_or(|f| m.url.as_ref().is_some_and(|url| url.folder == f)))
            .collect();
        match (matching.as_slice(), folder) {
            ([one], _) => Ok((one.block.clone(), one.move_to.clone())),
            ([], None) => raise("ArgumentError", "no mailbox to deliver to: declare `on_email url do |email| … end`"),
            ([], Some(f)) => raise("ArgumentError", format!("no `on_email` reads the folder {f:?}")),
            (_, None) => raise(
                "ArgumentError",
                "several mailboxes are declared: say which with `folder:` (the folder of its URL)",
            ),
            (_, Some(f)) => raise("ArgumentError", format!("several `on_email` read the folder {f:?}")),
        }
    }

    /// The message the named arguments describe, and its attachments.
    fn message_of(&self, args: &Args<'p>) -> Result<(Message, Vec<Value<'p>>), Ctrl<'p>> {
        let mut message = Message { date: Some(self.now()), ..Message::default() };
        let mut attachments = Vec::new();
        for (field, value) in &args.named {
            let text = || named_text(args, field).map(Option::unwrap_or_default);
            match field.as_str() {
                "folder" => {}
                "from" => message.from = Some(text()?),
                "from_name" => message.from_name = Some(text()?),
                "reply_to" => message.reply_to = Some(text()?),
                "message_id" => message.message_id = Some(text()?),
                "subject" => message.subject = text()?,
                "text" => message.text = text()?,
                "html" => message.html = Some(text()?),
                "to" => message.to = addresses(field, value)?,
                "cc" => message.cc = addresses(field, value)?,
                "date" => message.date = Some(instant(value)?),
                "attachments" => attachments = attachments_of(value)?,
                other => {
                    return raise(
                        "ArgumentError",
                        format!(
                            "`deliver_email` has no field `{other}:` (from, from_name, to, cc, reply_to, subject, date, message_id, text, html, attachments, folder)"
                        ),
                    );
                }
            }
        }
        if message.from.is_none() {
            return raise("ArgumentError", format!("`deliver_email` expects `from:`: `{USAGE}`"));
        }
        // a message with HTML only reads as text too, as a parsed one does
        if message.text.is_empty()
            && let Some(html) = &message.html
        {
            message.text = html.clone();
        }
        Ok((message, attachments))
    }
}

/// The text of a named argument, `None` when absent.
fn named_text<'p>(args: &Args<'p>, name: &str) -> Result<Option<String>, Ctrl<'p>> {
    match args.named.iter().find(|(k, _)| k == name).map(|(_, v)| v.untainted()) {
        None => Ok(None),
        Some(Value::Str(s)) => Ok(Some(s.to_string())),
        Some(other) => {
            raise("TypeError", format!("`deliver_email` expects `{name}:` a string, got {}", other.inspect()))
        }
    }
}

/// `to:` and `cc:`: an address, or several.
fn addresses<'p>(field: &str, value: &Value<'p>) -> Result<Vec<String>, Ctrl<'p>> {
    match value.untainted() {
        Value::Str(one) => Ok(vec![one.to_string()]),
        Value::Array(items) => items
            .borrow()
            .iter()
            .map(|item| match item.untainted() {
                Value::Str(s) => Ok(s.to_string()),
                other => raise("TypeError", format!("`{field}:` holds addresses, got {}", other.inspect())),
            })
            .collect(),
        other => raise("TypeError", format!("`{field}:` expects an address or several, got {}", other.inspect())),
    }
}

/// `date:`: epoch seconds, or ISO 8601 text.
fn instant<'p>(value: &Value<'p>) -> Result<f64, Ctrl<'p>> {
    match value.untainted() {
        Value::Str(text) => iso8601::parse(text).or_else(|e| raise("ArgumentError", e)),
        v if number(v).is_some() => Ok(number(v).expect("checked")),
        other => raise("TypeError", format!("`date:` expects an instant, got {}", other.inspect())),
    }
}

/// `attachments:`: what `Pdf.read`, `Image.read`, `Audio.read`… return.
fn attachments_of<'p>(value: &Value<'p>) -> Result<Vec<Value<'p>>, Ctrl<'p>> {
    let Value::Array(items) = value.untainted() else {
        return raise("TypeError", format!("`attachments:` expects an array, got {}", value.inspect()));
    };
    let items = items.borrow().clone();
    for item in &items {
        if !matches!(item.untainted(), Value::Record(r) if &*r.ty == ATTACHMENT) {
            return raise(
                "TypeError",
                format!("`attachments:` holds attachments (`Pdf.read(path)`…), got {}", item.inspect()),
            );
        }
    }
    Ok(items.into_iter().map(|item| item.untainted().clone()).collect())
}
