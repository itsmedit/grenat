//! Messages parsed from MIME fixtures: addresses and encoded words,
//! dates as epoch seconds, text and HTML (quoted-printable, base64, legacy
//! and CJK charsets), attachments with safe names, messages forwarded whole.

use grenat_imap::{Attachment, Message};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn parse(name: &str) -> Message {
    Message::parse(&fixture(name)).unwrap_or_else(|| panic!("{name} is a message"))
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
    assert_eq!(Message::parse(b""), None);
    assert_eq!(Message::parse(b"\r\n\r\njust a body"), None);
    // a header alone is a message, empty
    let m = Message::parse(b"Subject: hi\r\n\r\n").unwrap();
    assert_eq!((m.subject.as_str(), m.text.as_str(), m.from), ("hi", "", None));
}
