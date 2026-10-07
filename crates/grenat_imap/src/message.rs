//! A message, read: its RFC 5322 bytes parsed (MIME, every charset,
//! encoded words in headers) into what a program uses — addresses, the
//! subject, the date, the text and the HTML, the attachments. Nothing is
//! trusted here: everything comes from whoever sent the message. Its
//! structure is checked before it is parsed (see `shape`): a message that
//! would exhaust the parser's stack or memory is refused, never parsed.

use mail_parser::{Address, MessageParser, PartType};

use crate::attachment::Attachment;
use crate::malformed::Malformed;
use crate::shape::{self, MAX_NESTED, MAX_PARTS};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Message {
    /// The sender's address (`ada@acme.com`), if it has one.
    pub from: Option<String>,
    /// The sender's display name (`Ada Lovelace`), if given.
    pub from_name: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    /// Where replies go, when it is not the sender.
    pub reply_to: Option<String>,
    pub subject: String,
    /// When the sender says it was written, in seconds since the epoch
    /// (`Date:`, its offset applied); `None` when absent or not a date.
    pub date: Option<f64>,
    /// Without its angle brackets.
    pub message_id: Option<String>,
    /// The text parts, joined; the HTML made text when there is no text
    /// part. Lines end with `\n` (mail's `\r\n` made so), here and in `html`.
    pub text: String,
    /// The HTML parts, joined, when the message has any.
    pub html: Option<String>,
    /// The files, the inline images, the messages forwarded whole (whose
    /// own parts stay inside them).
    pub attachments: Vec<Attachment>,
}

impl Message {
    /// The message `raw` holds; [`Malformed`] when it holds none, or one
    /// whose structure is refused.
    pub fn parse(raw: &[u8]) -> Result<Message, Malformed> {
        let shape = shape::of(raw);
        if shape.encoded_message {
            return Err(Malformed::EncodedMessage);
        }
        if shape.nested > MAX_NESTED {
            return Err(Malformed::TooNested(shape.nested));
        }
        if shape.parts.is_none_or(|parts| parts > MAX_PARTS) {
            return Err(Malformed::TooManyParts(shape.parts));
        }
        let parsed = MessageParser::default().parse(raw).ok_or(Malformed::NoHeader)?;
        let read = Message::read(&parsed);
        dismantle(parsed);
        read
    }

    fn read(parsed: &mail_parser::Message) -> Result<Message, Malformed> {
        if parsed.headers().is_empty() {
            return Err(Malformed::NoHeader);
        }
        if parsed.parts.len() > MAX_PARTS {
            return Err(Malformed::TooManyParts(Some(parsed.parts.len())));
        }
        let sender = parsed.from().and_then(Address::first);
        let texts: Vec<String> =
            (0..parsed.text_body_count()).filter_map(|i| parsed.body_text(i)).map(|t| lines(&t)).collect();
        let htmls: Vec<String> = parsed
            .html_bodies()
            .filter(|part| part.is_text_html())
            .filter_map(|part| part.text_contents())
            .map(lines)
            .collect();
        Ok(Message {
            from: sender.and_then(|a| a.address()).map(str::to_string),
            from_name: sender.and_then(|a| a.name()).map(str::to_string),
            to: addresses(parsed.to()),
            cc: addresses(parsed.cc()),
            reply_to: parsed.reply_to().and_then(Address::first).and_then(|a| a.address()).map(str::to_string),
            subject: parsed.subject().unwrap_or_default().to_string(),
            date: parsed.date().filter(|d| d.is_valid()).map(|d| d.to_timestamp() as f64),
            message_id: parsed.message_id().map(str::to_string),
            text: texts.join("\n"),
            html: (!htmls.is_empty()).then(|| htmls.join("\n")),
            attachments: parsed.attachments().map(Attachment::from_part).collect(),
        })
    }
}

/// Frees a parsed message without recursion: `mail-parser` frees a message
/// nested in another by recursion, one stack frame per level.
fn dismantle(parsed: mail_parser::Message) {
    let mut pending = vec![parsed];
    while let Some(mut message) = pending.pop() {
        for part in &mut message.parts {
            if let PartType::Message(nested) = std::mem::take(&mut part.body) {
                pending.push(nested);
            }
        }
    }
}

/// `text` with `\n` line ends.
fn lines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Every address of a header (groups flattened).
fn addresses(address: Option<&Address>) -> Vec<String> {
    address.map(|a| a.iter().filter_map(|a| a.address()).map(str::to_string).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deeply_nested_message_is_freed_without_recursion() {
        let mut raw = b"From: e@v.il\r\n".to_vec();
        raw.extend(b"Content-Type: message/rfc822\r\n\r\nFrom: a@b.c\r\n".repeat(50_000));
        // on a small stack, which recursion over 50,000 levels would overflow
        let freed = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let parsed = MessageParser::default().parse(&raw[..]).expect("a message");
                dismantle(parsed);
            })
            .unwrap()
            .join();
        assert!(freed.is_ok());
    }
}
