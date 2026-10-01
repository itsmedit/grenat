//! Audio for models: the formats Grenat knows — a file's extension, its
//! media type, the name providers give the format (`mp3`, `wav`…) — and
//! the audio an attachment carries in base64: its size, its bytes, whether
//! a request holds any.

use serde_json::Value as Json;

use crate::LlmError;

/// (extension, media type, format name), the first of a media type naming it.
const FORMATS: &[(&str, &str, &str)] = &[
    ("mp3", "audio/mpeg", "mp3"),
    ("mpeg", "audio/mpeg", "mp3"),
    ("mpga", "audio/mpeg", "mp3"),
    ("wav", "audio/wav", "wav"),
    ("m4a", "audio/mp4", "m4a"),
    ("mp4", "audio/mp4", "m4a"),
    ("ogg", "audio/ogg", "ogg"),
    ("oga", "audio/ogg", "ogg"),
    ("opus", "audio/opus", "opus"),
    ("flac", "audio/flac", "flac"),
    ("webm", "audio/webm", "webm"),
    ("aac", "audio/aac", "aac"),
    ("aiff", "audio/aiff", "aiff"),
    ("aif", "audio/aiff", "aiff"),
];

/// Other names servers give these media types (`Content-Type`).
const ALIASES: &[(&str, &str)] = &[
    ("audio/mp3", "audio/mpeg"),
    ("audio/x-wav", "audio/wav"),
    ("audio/wave", "audio/wav"),
    ("audio/vnd.wave", "audio/wav"),
    ("audio/x-m4a", "audio/mp4"),
    ("audio/m4a", "audio/mp4"),
    ("video/mp4", "audio/mp4"),
    ("audio/x-flac", "audio/flac"),
    ("audio/x-aiff", "audio/aiff"),
    ("video/webm", "audio/webm"),
    ("application/ogg", "audio/ogg"),
];

/// The media type of an audio file, by its extension (any case).
pub fn media_type_of_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let extension = name.rsplit_once('.')?.1.to_lowercase();
    FORMATS.iter().find(|(e, ..)| *e == extension).map(|&(_, media_type, _)| media_type)
}

/// The media type a `Content-Type` header names, if it is audio Grenat knows.
pub fn media_type_of_header(content_type: &str) -> Option<&'static str> {
    let essence = content_type.split(';').next().unwrap_or_default().trim().to_lowercase();
    let essence = ALIASES.iter().find(|(alias, _)| *alias == essence).map_or(essence.as_str(), |&(_, m)| m);
    FORMATS.iter().find(|(_, m, _)| *m == essence).map(|&(_, media_type, _)| media_type)
}

/// The name providers give the format of `media_type` (`mp3` for `audio/mpeg`).
pub fn format_of(media_type: &str) -> Option<&'static str> {
    let media_type = media_type_of_header(media_type)?;
    FORMATS.iter().find(|(_, m, _)| *m == media_type).map(|&(.., format)| format)
}

/// The extensions Grenat reads as audio, for messages.
pub fn extensions() -> String {
    let all: Vec<String> = FORMATS.iter().map(|(e, ..)| format!(".{e}")).collect();
    all.join(", ")
}

/// The size of the bytes `base64` stands for.
pub fn decoded_len(base64: &str) -> usize {
    let padding = base64.bytes().rev().take_while(|b| *b == b'=').count();
    (base64.len() / 4 * 3).saturating_sub(padding)
}

/// A size, in megabytes as providers count them (2^20 bytes).
pub fn megabytes(bytes: usize) -> String {
    format!("{:.1} MB", bytes as f64 / crate::catalog::MB as f64)
}

/// The bytes of standard, padded base64 (as attachments carry them).
pub fn decode_base64(text: &str) -> Result<Vec<u8>, LlmError> {
    fn value(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let invalid = || LlmError::new("the audio's data is not base64");
    let bytes = text.trim_end_matches('=').as_bytes();
    if !text.len().is_multiple_of(4) || text.len() - bytes.len() > 2 {
        return Err(invalid());
    }
    let mut out = Vec::with_capacity(decoded_len(text));
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= value(*c).ok_or_else(invalid)? << (18 - 6 * i);
        }
        let produced = chunk.len() * 6 / 8;
        out.extend((0..produced).map(|i| (n >> (16 - 8 * i)) as u8));
    }
    Ok(out)
}

/// Whether a message of `messages` carries audio.
pub fn in_messages(messages: &[Json]) -> bool {
    messages.iter().any(|m| m["content"].as_array().is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "audio")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_by_extension_and_by_header() {
        assert_eq!(media_type_of_path("rec/Meeting.MP3"), Some("audio/mpeg"));
        assert_eq!(media_type_of_path("a.m4a"), Some("audio/mp4"));
        assert_eq!(media_type_of_path("notes.txt"), None);
        assert_eq!(media_type_of_path("mp3"), None);
        assert_eq!(media_type_of_header("audio/x-wav; charset=binary"), Some("audio/wav"));
        assert_eq!(media_type_of_header("text/html"), None);
        assert_eq!(format_of("audio/mpeg"), Some("mp3"));
        assert_eq!(format_of("audio/mp4"), Some("m4a"));
        assert_eq!(format_of("audio/wave"), Some("wav"));
        assert!(extensions().starts_with(".mp3, .mpeg"));
    }

    #[test]
    fn base64_is_measured_and_decoded() {
        for (text, bytes) in
            [("", &b""[..]), ("Zg==", b"f"), ("Zm8=", b"fo"), ("Zm9v", b"foo"), ("//4A", &[0xff, 0xfe, 0])]
        {
            assert_eq!(decode_base64(text).unwrap(), bytes, "{text}");
            assert_eq!(decoded_len(text), bytes.len(), "{text}");
        }
        assert!(decode_base64("Zm9").is_err());
        assert!(decode_base64("Zm9*").is_err());
        assert!(decode_base64("Z===").is_err());
        assert_eq!(megabytes(26_214_400), "25.0 MB");
    }

    #[test]
    fn audio_is_found_in_messages() {
        let audio = json!({"role": "user", "content": [{"type": "audio", "source": {}}]});
        assert!(in_messages(&[json!({"role": "user", "content": "hi"}), audio]));
        assert!(!in_messages(&[json!({"role": "user", "content": [{"type": "text", "text": "hi"}]})]));
    }
}
