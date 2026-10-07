//! Folder names as IMAP4rev1 writes them: modified UTF-7 (RFC 3501,
//! section 5.1.3). Printable ASCII stands for itself, `&` is `&-`, any
//! other run of characters is `&` + its UTF-16 in base64 (`,` for `/`,
//! no padding) + `-`. `Boîte` is `Bo&AO4-te`.

const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

/// A folder name, encoded.
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut out, &mut pending);
            match c {
                '&' => out.push_str("&-"),
                c => out.push(c),
            }
        } else {
            pending.extend(c.encode_utf16(&mut [0; 2]).iter());
        }
    }
    flush(&mut out, &mut pending);
    out
}

/// A folder name encoded and quoted, for a command the library does not
/// quote itself (`UID COPY`): `"`, `\\` escaped. Encoded, the name holds no
/// line break that could end the command.
pub(crate) fn quoted(name: &str) -> String {
    let encoded = encode(name);
    format!("\"{}\"", encoded.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Writes a run of other characters: `&`, base64, `-`.
fn flush(out: &mut String, pending: &mut Vec<u16>) {
    if pending.is_empty() {
        return;
    }
    let bytes: Vec<u8> = pending.drain(..).flat_map(u16::to_be_bytes).collect();
    out.push('&');
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out.push('-');
}

#[cfg(test)]
mod tests {
    use super::{encode, quoted};

    #[test]
    fn modified_utf7() {
        assert_eq!(encode("INBOX"), "INBOX");
        assert_eq!(encode("Tom & Jerry"), "Tom &- Jerry");
        assert_eq!(encode("Boîte"), "Bo&AO4-te");
        // RFC 3501's example
        assert_eq!(encode("~peter/mail/台北/日本語"), "~peter/mail/&U,BTFw-/&ZeVnLIqe-");
        // outside the BMP: a surrogate pair
        assert_eq!(encode("📬"), "&2D3c7A-");
    }

    #[test]
    fn quoted_names() {
        assert_eq!(quoted("Done"), r#""Done""#);
        assert_eq!(quoted(r#"Say "hi" \ bye"#), r#""Say \"hi\" \\ bye""#);
        // a line break is encoded, never sent as one
        assert_eq!(quoted("a\r\nb"), r#""a&AA0ACg-b""#);
    }
}
