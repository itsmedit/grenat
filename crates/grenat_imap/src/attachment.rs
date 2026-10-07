//! An attachment: its name, made safe to use as a file name, its media
//! type and its bytes, decoded. The sender chose all three: the name may
//! try to climb directories (`../../etc/passwd`), hide its extension
//! behind control characters, or be no name at all.

use mail_parser::{MessagePart, MimeHeaders, PartType};

/// The longest name kept, in bytes (what most file systems take).
const MAX_NAME: usize = 255;

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    /// A file name without any directory (see [`sanitize_name`]); `None`
    /// when the sender gave none that survives.
    pub name: Option<String>,
    /// As the sender declared it (`application/pdf`), lowercased, without
    /// parameters; `message/rfc822` for a message forwarded whole.
    pub media_type: String,
    /// Decoded (base64, quoted-printable); a forwarded message's RFC 5322 bytes.
    pub bytes: Vec<u8>,
}

impl Attachment {
    pub(crate) fn from_part(part: &MessagePart) -> Attachment {
        Attachment {
            name: part.attachment_name().and_then(sanitize_name),
            media_type: media_type(part),
            bytes: part.contents().to_vec(),
        }
    }
}

fn media_type(part: &MessagePart) -> String {
    let declared = part.content_type().map(|ct| match ct.subtype() {
        Some(sub) => format!("{}/{}", ct.ctype(), sub),
        None => ct.ctype().to_string(),
    });
    let media_type = declared.unwrap_or_else(|| {
        match part.body {
            PartType::Message(_) => "message/rfc822",
            PartType::Text(_) => "text/plain",
            PartType::Html(_) => "text/html",
            _ => "application/octet-stream",
        }
        .to_string()
    });
    media_type.to_ascii_lowercase()
}

/// The last component of `name` (after any `/` or `\`), without control
/// characters nor bidirectional marks, surrounding spaces or leading dots (no hidden file, no
/// `..`), cut to 255 bytes keeping its extension; `None` if nothing is left.
pub fn sanitize_name(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last.chars().filter(|c| !c.is_control() && !is_bidi_control(*c)).collect();
    let clean = clean.trim().trim_start_matches('.').trim();
    if clean.is_empty() {
        return None;
    }
    Some(shortened(clean))
}

/// The marks that reorder text (`invoice\u{202e}fdp.exe` shows as
/// `invoiceexe.pdf`): no name needs them.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// `name` within [`MAX_NAME`] bytes, its extension (if short) kept.
fn shortened(name: &str) -> String {
    if name.len() <= MAX_NAME {
        return name.to_string();
    }
    let extension = name.rfind('.').map(|i| &name[i..]).filter(|e| e.len() <= 16).unwrap_or("");
    let mut stem = &name[..name.len() - extension.len()];
    let mut end = MAX_NAME - extension.len();
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    stem = &stem[..end];
    format!("{stem}{extension}")
}

#[cfg(test)]
mod tests {
    use super::sanitize_name;

    #[test]
    fn names_lose_their_directories() {
        assert_eq!(sanitize_name("invoice.pdf").as_deref(), Some("invoice.pdf"));
        assert_eq!(sanitize_name("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(sanitize_name(r"C:\Users\ada\report.docx").as_deref(), Some("report.docx"));
        assert_eq!(sanitize_name("..").as_deref(), None);
        assert_eq!(sanitize_name("/").as_deref(), None);
        assert_eq!(sanitize_name("  ").as_deref(), None);
        assert_eq!(sanitize_name(".bashrc").as_deref(), Some("bashrc"));
        assert_eq!(sanitize_name("evil\u{202e}fdp.exe").as_deref(), Some("evilfdp.exe"));
        assert_eq!(sanitize_name("a\r\nb\0.txt").as_deref(), Some("ab.txt"));
        assert_eq!(sanitize_name("合同 2026.pdf").as_deref(), Some("合同 2026.pdf"));
    }

    #[test]
    fn long_names_keep_their_extension() {
        let name = sanitize_name(&format!("{}.pdf", "é".repeat(300))).unwrap();
        assert!(name.len() <= 255 && name.ends_with("é.pdf"), "{name}");
    }
}
