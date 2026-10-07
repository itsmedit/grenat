//! Messages parsed from MIME fixtures: addresses and encoded words,
//! dates as epoch seconds, text and HTML (quoted-printable, base64, legacy
//! and CJK charsets), attachments with safe names, messages forwarded whole;
//! and crafted messages refused before parsing (nested too deep, encoded
//! forwarded messages, too many parts), on a stack as small as a mailbox
//! task's.

use grenat_imap::{Attachment, MAX_NESTED, MAX_PARTS, Malformed, Message};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn parse(name: &str) -> Message {
    Message::parse(&fixture(name)).unwrap_or_else(|e| panic!("{name} is a message: {e}"))
}

#[test]
fn a_plain_message() {
    let m = parse("plain.eml");
    assert_eq!(m.from.as_deref(), Some("ada@acme.com"));
    assert_eq!(m.from_name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(m.to, ["support@acme.com", "grace@acme.com"]);
    assert_eq!(m.cc, ["audit@acme.com"]);
    assert_eq!(m.reply_to.as_deref(), Some("tickets@acme.com"));
    assert_eq!(m.subject, "Invoice 42 is late");
    // 09:30 at +02:00 is 07:30 UTC
    assert_eq!(m.date, Some(1_790_839_800.0));
    assert_eq!(m.message_id.as_deref(), Some("42.ada@acme.com"));
    // mail's CRLF line ends become `\n`
    assert_eq!(m.text, "Hello,\nthe invoice 42 has not arrived.\n");
    assert_eq!(m.html, None);
    assert!(m.attachments.is_empty());
}

#[test]
fn alternative_parts_in_quoted_printable() {
    let m = parse("alternative.eml");
    assert_eq!(m.from_name.as_deref(), Some("Renée Dupré"));
    assert_eq!(m.subject, "Re\u{301}glement déjà payé");
    // 18:05:09 at -04:00 is 22:05:09 UTC
    assert_eq!(m.date, Some(1_790_978_709.0));
    assert_eq!(m.text.trim_end(), "Café crème, déjà payé — merci");
    assert_eq!(m.html.as_deref().map(str::trim_end), Some("<p>Café crème, déjà payé — <b>merci</b></p>"));
    assert!(m.attachments.is_empty());
}

#[test]
fn html_alone_gives_its_text_too() {
    let m = parse("html_only.eml");
    assert!(m.html.as_deref().unwrap().contains("<h1>Weekly</h1>"));
    assert!(m.text.contains("Weekly") && m.text.contains("Nothing new.") && !m.text.contains('<'), "{}", m.text);
    assert_eq!(m.date, Some(1_791_014_400.0));
    assert_eq!(m.from_name, None);
}

#[test]
fn attachments_decoded_and_named_safely() {
    let m = parse("attachments.eml");
    assert_eq!(m.html.as_deref().map(str::trim_end), Some(r#"<p>See <img src="cid:logo"></p>"#));
    let summary: Vec<(Option<&str>, &str, usize)> =
        m.attachments.iter().map(|a| (a.name.as_deref(), a.media_type.as_str(), a.bytes.len())).collect();
    assert_eq!(
        summary,
        [
            (Some("logo.png"), "image/png", 33),
            // RFC 2231's `filename*` wins over `name`, UTF-8 decoded; the type lowercased
            (Some("合同 2026.pdf"), "application/pdf", 45),
            // no directory survives
            (Some("run.sh"), "application/x-sh", 19),
            (None, "application/octet-stream", 3),
        ]
    );
    let pdf: &Attachment = &m.attachments[1];
    assert!(pdf.bytes.starts_with(b"%PDF-1.4\n") && pdf.bytes.ends_with(b"%%EOF\n"));
    assert_eq!(m.attachments[2].bytes, b"#!/bin/sh\nrm -rf ~\n");
}

#[test]
fn a_forwarded_message_stays_whole() {
    let m = parse("nested.eml");
    assert_eq!(m.text.trim_end(), "Forwarding this one.");
    assert_eq!(m.attachments.len(), 1);
    let forwarded = &m.attachments[0];
    assert_eq!((forwarded.name.as_deref(), forwarded.media_type.as_str()), (None, "message/rfc822"));
    let inner = Message::parse(&forwarded.bytes).unwrap();
    assert_eq!(inner.subject, "The original");
    assert_eq!(inner.from.as_deref(), Some("customer@example.com"));
    assert_eq!(inner.text.trim_end(), "Inner text, not the outer one.");
}

#[test]
fn cjk_charsets() {
    let m = parse("cjk.eml");
    // UTF-8 base64 name, GB2312 subject, EUC-KR cc
    assert_eq!(m.from_name.as_deref(), Some("王伟"));
    assert_eq!(m.subject, "发票问题");
    assert_eq!(m.cc, ["kim@example.kr"]);
    // ISO-2022-JP body
    assert_eq!(m.text.trim_end(), "請求書をお送りします。");
    // 09:00 at +08:00 is 01:00 UTC
    assert_eq!(m.date, Some(1_791_248_400.0));
    // a Big5 encoded word as the name; a Shift_JIS text attachment, decoded to UTF-8
    let attachment = &m.attachments[0];
    assert_eq!(attachment.name.as_deref(), Some("報價單.txt"));
    assert_eq!(attachment.media_type, "text/plain");
    assert_eq!(String::from_utf8_lossy(&attachment.bytes), "見積もり");
}

#[test]
fn legacy_charsets_and_a_bad_date() {
    let m = parse("latin1.eml");
    assert_eq!(m.subject, "Señor Muñoz");
    assert_eq!(m.text.trim_end(), "Mañana € 100, “gracias”.");
    assert_eq!(m.date, None);
    assert_eq!(m.message_id, None);
}

#[test]
fn what_is_no_message() {
    assert_eq!(Message::parse(b""), Err(Malformed::NoHeader));
    assert_eq!(Message::parse(b"\r\n\r\njust a body"), Err(Malformed::NoHeader));
    assert_eq!(Malformed::NoHeader.to_string(), "it is not an email (no header)");
    // a header alone is a message, empty
    let m = Message::parse(b"Subject: hi\r\n\r\n").unwrap();
    assert_eq!((m.subject.as_str(), m.text.as_str(), m.from), ("hi", "", None));
}

// ── Crafted messages ──────────────────────────────────────────────────

/// Parses `raw` on a thread with a mailbox task's stack (16 MiB): a stack
/// overflow there would abort the whole process.
fn parse_on_a_task_stack(raw: Vec<u8>) -> Result<Message, Malformed> {
    std::thread::Builder::new().stack_size(16 << 20).spawn(move || Message::parse(&raw)).unwrap().join().unwrap()
}

/// `levels` messages, each in the one before (`message/rfc822`, unencoded).
fn nested(levels: usize) -> Vec<u8> {
    let mut raw = b"From: eve@evil.example\r\nSubject: hi\r\n".to_vec();
    raw.extend(b"Content-Type: message/rfc822\r\n\r\nFrom: a@b.c\r\n".repeat(levels));
    raw.extend(b"\r\nhi\r\n");
    raw
}

#[test]
fn a_message_nested_too_deeply_is_refused() {
    // 15 MB of nesting: freeing it alone overflowed a 16 MiB stack
    let raw = nested(15_000_000 / 45);
    assert!(raw.len() > 15_000_000);
    let e = parse_on_a_task_stack(raw).unwrap_err();
    assert!(matches!(e, Malformed::TooNested(n) if n > MAX_NESTED), "{e:?}");
    assert!(e.to_string().contains("over the limit of 100"), "{e}");
    // within the limit, the outer message reads as any other
    let m = parse_on_a_task_stack(nested(MAX_NESTED - 1)).unwrap();
    assert_eq!((m.subject.as_str(), m.attachments.len()), ("hi", 1));
    assert_eq!(m.attachments[0].media_type, "message/rfc822");
}

/// Base64 lines of 76 characters.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(3).enumerate() {
        if i > 0 && i % 19 == 0 {
            out.push_str("\r\n");
        }
        let n = chunk.iter().enumerate().fold(0u32, |n, (j, b)| n | u32::from(*b) << (16 - 8 * j));
        for j in 0..4 {
            out.push(if j <= chunk.len() { ALPHABET[(n >> (18 - 6 * j) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[test]
fn a_forwarded_message_encoded_is_refused() {
    // its nesting hides in base64: 20,000 levels in 1 MB, copied by recursion
    let mut raw = b"From: eve@evil.example\r\nSubject: hi\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: message/rfc822\r\nContent-Transfer-Encoding: base64\r\n\r\n".to_vec();
    raw.extend(base64(&nested(20_000)).as_bytes());
    raw.extend(b"\r\n--b--\r\n");
    assert!(raw.len() < 1_300_000);
    let e = parse_on_a_task_stack(raw).unwrap_err();
    assert_eq!(e, Malformed::EncodedMessage);
    assert!(e.to_string().contains("cannot be checked before parsing"), "{e}");
}

/// A `multipart/mixed` message of `parts` tiny parts.
fn tiny_parts(parts: usize) -> Vec<u8> {
    let mut raw =
        b"From: eve@evil.example\r\nSubject: hi\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n".to_vec();
    raw.extend(b"--b\r\nContent-Type: a/b\r\n\r\nx\r\n".repeat(parts));
    raw.extend(b"--b--\r\n");
    raw
}

#[test]
fn a_message_of_too_many_parts_is_refused() {
    // 24 MB of parts: parsed, it took 700 MB, then 1.3 GB as values
    let e = parse_on_a_task_stack(tiny_parts(24_000_000 / 30)).unwrap_err();
    assert!(matches!(e, Malformed::TooManyParts(Some(n)) if n > MAX_PARTS), "{e:?}");
    assert!(e.to_string().contains("over the limit of 1000"), "{e}");
    // within the limit: every part is an attachment
    let m = parse_on_a_task_stack(tiny_parts(MAX_PARTS - 2)).unwrap();
    assert_eq!(m.attachments.len(), MAX_PARTS - 2);
}
