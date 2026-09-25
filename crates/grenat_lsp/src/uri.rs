//! `file://` URIs and paths.

use std::path::{Path, PathBuf};

/// The path of a `file://` URI.
pub(crate) fn to_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(path.len());
    let mut chars = path.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hex = [chars.next()?, chars.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    let path = PathBuf::from(String::from_utf8(bytes).ok()?);
    // the same path as the loader sees it
    Some(path.canonicalize().unwrap_or(path))
}

/// The `file://` URI of a path.
pub(crate) fn from_path(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut uri = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        let path = Path::new("/no/such dir/é.grn");
        let uri = from_path(path);
        assert_eq!(uri, "file:///no/such%20dir/%C3%A9.grn");
        assert_eq!(to_path(&uri).unwrap(), path);
        assert_eq!(to_path("https://x"), None);
    }
}
