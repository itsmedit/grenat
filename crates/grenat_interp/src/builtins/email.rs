//! `on_email`: a mailbox that wakes a served program.
//!
//! ```ruby
//! on_email Credentials.fetch(:support, :imap_url), every: 1.minute, move_to: "Done" do |email|
//!   reply(email.from, triage(email.subject, email.text))   # all of `email` is untrusted
//! end
//! ```
//!
//! The URL (`imaps://user:app-password@imap.gmail.com/INBOX`, a `String`
//! or a `Secret`) is parsed and its host held to the `net` capabilities
//! when the declaration runs; the password only ever reaches the
//! connection. `grenat serve` polls the mailbox (see `crate::mailboxes`);
//! in tests, `deliver_email` hands the handler a message (see
//! `eval::email_double`). What a message carries is untrusted: every field
//! of the `IncomingEmail` record, its attachments included.

use grenat_imap::{MailboxUrl, Message};

use crate::prelude::*;

use super::*;

/// The record an `on_email` handler receives. (`Email` already names an
/// address, a `String`.)
pub(crate) const INCOMING_EMAIL: &str = "IncomingEmail";

/// How often a mailbox is read when `every:` is not given.
const DEFAULT_EVERY: f64 = 60.0;

/// A mailbox declared by `on_email`.
pub(crate) struct EmailTrigger<'p> {
    /// `None` in tests, for a stand-in credential that names no mailbox.
    pub url: Option<MailboxUrl>,
    /// Seconds between two reads.
    pub every: f64,
    /// Where a handled message goes; `None`: it stays, marked seen.
    pub move_to: Option<String>,
    /// The function giving an OAuth 2.0 access token (XOAUTH2), called
    /// before each read; `None`: the URL's password.
    pub token: Option<String>,
    pub block: Value<'p>,
}

impl EmailTrigger<'_> {
    /// How logs and events name the mailbox: never its user nor its password.
    pub(crate) fn label(&self) -> String {
        match &self.url {
            Some(url) => url.label(),
            None => "a test mailbox".into(),
        }
    }
}

pub(crate) fn on_email<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    if args.pos.first().is_some_and(Value::contains_taint) {
        return raise("TaintError", "an untrusted value reaches `on_email` (effect `net`) without validation");
    }
    let text = text_arg(args, 0, "on_email")?;
    let url = match MailboxUrl::parse(&text) {
        Ok(url) => Some(url),
        // `Credentials.fetch` in a test without credentials: a stand-in, never read
        Err(_) if interp.offline && matches!(args.pos[0].untainted(), Value::Secret(_)) => None,
        Err(e) => return raise("ArgumentError", e.message().to_string()),
    };
    if let Some(url) = &url {
        interp.check_net(&url.host, &format!("imaps://{}", url.label()))?;
    }
    let mut trigger =
        EmailTrigger { url, every: DEFAULT_EVERY, move_to: None, token: None, block: block(args, "on_email")? };
    for (option, value) in &args.named {
        match (option.as_str(), value.untainted()) {
            ("every", v) if number(v).is_some_and(|s| s > 0.0 && s.is_finite()) => {
                trigger.every = number(v).expect("checked");
            }
            ("move_to", Value::Str(folder)) if !folder.trim().is_empty() => trigger.move_to = Some(folder.to_string()),
            ("token", Value::Symbol(function)) if interp.fns.contains_key(&**function) => {
                trigger.token = Some(function.to_string());
            }
            ("token", Value::Symbol(function)) => {
                return raise("NameError", format!("`on_email` takes `token:` a function: no function `{function}`"));
            }
            (option, v) => {
                return raise(
                    "ArgumentError",
                    format!(
                        "invalid `on_email` option `{option}: {}` (it takes `every:` a duration, `move_to:` a folder, `token:` a function)",
                        v.inspect()
                    ),
                );
            }
        }
    }
    interp.mailboxes.borrow_mut().push(trigger);
    Ok(Value::Nil)
}

/// A message as its handler sees it: every field untrusted. `attachments`
/// are `Attachment` records (made by Grenat, so a model takes them).
pub(crate) fn incoming_email<'p>(message: &Message, attachments: Vec<Value<'p>>) -> Value<'p> {
    let text = |s: &str| Value::str(s).taint();
    let maybe = |s: &Option<String>| s.as_deref().map_or(Value::Nil, text);
    let list = |items: &[String]| Value::array(items.iter().map(|s| text(s)).collect()).taint();
    Value::record(
        INCOMING_EMAIL,
        vec![
            ("from".into(), maybe(&message.from)),
            ("from_name".into(), maybe(&message.from_name)),
            ("to".into(), list(&message.to)),
            ("cc".into(), list(&message.cc)),
            ("reply_to".into(), maybe(&message.reply_to)),
            ("subject".into(), text(&message.subject)),
            ("date".into(), message.date.map_or(Value::Nil, |d| Value::Float(d).taint())),
            ("message_id".into(), maybe(&message.message_id)),
            ("text".into(), text(&message.text)),
            ("html".into(), maybe(&message.html)),
            ("attachments".into(), Value::array(attachments.into_iter().map(Value::taint).collect()).taint()),
        ],
    )
}

/// An attachment received: a PDF is a `document`, a PNG, JPEG, GIF or
/// WebP an `image`, audio Grenat knows `audio` (a model takes those); the
/// rest a `file`, kept with its media type.
pub(crate) fn received_attachment<'p>(received: &grenat_imap::Attachment) -> Value<'p> {
    let declared = received.media_type.as_str();
    let (kind, media_type) = match declared {
        "application/pdf" => ("document", declared),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => ("image", declared),
        _ => match grenat_llm::audio::media_type_of_header(declared) {
            Some(audio) => ("audio", audio),
            None => ("file", declared),
        },
    };
    attachment(kind, media_type, "base64", base64(&received.bytes), received.name.as_deref())
}

/// What a message read from a mailbox gives its handler.
pub(crate) fn email_value<'p>(message: &Message) -> Value<'p> {
    incoming_email(message, message.attachments.iter().map(received_attachment).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_of(media_type: &str) -> (String, String) {
        let received = grenat_imap::Attachment { name: None, media_type: media_type.into(), bytes: b"x".to_vec() };
        let Value::Record(r) = received_attachment(&received) else { panic!("a record") };
        let get = |n: &str| r.fields.iter().find(|(k, _)| &**k == n).map(|(_, v)| v.to_display()).unwrap();
        (get("kind"), get("media_type"))
    }

    #[test]
    fn attachments_are_sorted_by_what_a_model_reads() {
        assert_eq!(kind_of("application/pdf"), ("document".into(), "application/pdf".into()));
        assert_eq!(kind_of("image/webp"), ("image".into(), "image/webp".into()));
        assert_eq!(kind_of("audio/mpeg"), ("audio".into(), "audio/mpeg".into()));
        assert_eq!(kind_of("application/zip"), ("file".into(), "application/zip".into()));
        assert_eq!(kind_of("message/rfc822"), ("file".into(), "message/rfc822".into()));
        assert_eq!(kind_of("image/svg+xml"), ("file".into(), "image/svg+xml".into()));
    }

    #[test]
    fn every_field_is_untrusted() {
        let message = Message {
            from: Some("ada@acme.com".into()),
            to: vec!["support@acme.com".into()],
            subject: "Hi".into(),
            date: Some(1.0),
            text: "Body".into(),
            attachments: vec![grenat_imap::Attachment {
                name: Some("a.pdf".into()),
                media_type: "application/pdf".into(),
                bytes: b"%PDF".to_vec(),
            }],
            ..Message::default()
        };
        let Value::Record(r) = email_value(&message) else { panic!("a record") };
        assert_eq!(&*r.ty, INCOMING_EMAIL);
        for (name, value) in &r.fields {
            assert!(matches!(value, Value::Nil) || value.is_tainted(), "`{name}` is trusted");
        }
    }
}
