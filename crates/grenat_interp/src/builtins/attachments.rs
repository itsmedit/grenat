//! Attachments: PDF documents and images given to a model.
//!
//! ```ruby
//! prompt extract(invoice: Attachment) -> ~Invoice using :fast
//!   user "Extract this invoice.", invoice
//! end
//! extract(Pdf.read("invoices/a.pdf"))    # Image.read("scan.png"), Pdf.url("https://…")
//! ```
//!
//! Reading a file is an `fs.read` effect; a URL is fetched by the model's
//! provider, not by the program.

use serde_json::{Value as Json, json};

use crate::prelude::*;

use super::*;

/// The record `Pdf.read`, `Image.read`… return.
pub(crate) const ATTACHMENT: &str = "Attachment";

pub(crate) fn call_attachment<'p>(interp: &mut Interp<'p>, module: &str, name: &str, args: &Args<'p>) -> R<'p> {
    let target = str_arg(args, 0, name)?.to_string();
    let kind = if module == "Pdf" { "document" } else { "image" };
    match name {
        "read" => {
            interp.check_fs("fs.read", &target)?;
            let media_type = media_type(module, &target)?;
            let bytes = std::fs::read(&target).or_else(|e| raise("IoError", format!("reading `{target}`: {e}")))?;
            Ok(attachment(kind, &media_type, "base64", base64(&bytes)))
        }
        "url" if target.starts_with("https://") || target.starts_with("http://") => {
            Ok(attachment(kind, "", "url", target))
        }
        "url" => raise("ArgumentError", format!("`{module}.url` expects an http(s) URL, got {target:?}")),
        _ => raise("NoMethodError", format!("unknown method `{module}.{name}`")),
    }
}

fn media_type<'p>(module: &str, path: &str) -> Result<String, Ctrl<'p>> {
    let extension = path.rsplit('.').next().unwrap_or_default().to_lowercase();
    Ok(match (module, extension.as_str()) {
        ("Pdf", _) => "application/pdf",
        (_, "png") => "image/png",
        (_, "jpg" | "jpeg") => "image/jpeg",
        (_, "gif") => "image/gif",
        (_, "webp") => "image/webp",
        _ => return raise("ArgumentError", format!("`{path}`: images are .png, .jpg, .gif or .webp")),
    }
    .to_string())
}

fn attachment<'p>(kind: &str, media_type: &str, source: &str, data: String) -> Value<'p> {
    Value::record(
        ATTACHMENT,
        vec![
            ("kind".into(), Value::str(kind)),
            ("media_type".into(), Value::str(media_type)),
            ("source".into(), Value::str(source)),
            ("data".into(), Value::str(data)),
        ],
    )
}

/// The content block of an attachment, as the Messages API takes it.
pub(crate) fn attachment_block(fields: &Fields) -> Json {
    let get = |n: &str| fields.iter().find(|(k, _)| &**k == n).map(|(_, v)| v.to_display()).unwrap_or_default();
    let source = match get("source").as_str() {
        "url" => json!({"type": "url", "url": get("data")}),
        _ => json!({"type": "base64", "media_type": get("media_type"), "data": get("data")}),
    };
    json!({"type": get("kind"), "source": source})
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_encoding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }
}
