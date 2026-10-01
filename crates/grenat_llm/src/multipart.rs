//! `multipart/form-data` bodies (RFC 7578): text fields and a file, as a
//! transcription API takes an upload. The boundary is one no part contains.

/// A form being written: its parts, then its body.
#[derive(Debug, Default)]
pub(crate) struct Form {
    parts: Vec<Part>,
}

/// A field: its name, its file name and media type if it is a file, its content.
type Part = (String, Option<(String, String)>, Vec<u8>);

impl Form {
    pub(crate) fn text(&mut self, name: &str, value: &str) {
        self.parts.push((name.to_string(), None, value.as_bytes().to_vec()));
    }

    pub(crate) fn file(&mut self, name: &str, filename: &str, media_type: &str, content: Vec<u8>) {
        self.parts.push((name.to_string(), Some((filename.to_string(), media_type.to_string())), content));
    }

    /// The `Content-Type` header and the body.
    pub(crate) fn finish(self) -> (String, Vec<u8>) {
        let boundary = self.boundary();
        let mut body = Vec::new();
        for (name, file, content) in self.parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            let disposition = format!("Content-Disposition: form-data; name=\"{}\"", quoted(&name));
            match file {
                Some((filename, media_type)) => body.extend_from_slice(
                    format!(
                        "{disposition}; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                        quoted(&filename),
                        one_line(&media_type)
                    )
                    .as_bytes(),
                ),
                None => body.extend_from_slice(format!("{disposition}\r\n\r\n").as_bytes()),
            }
            body.extend_from_slice(&content);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    /// A boundary found in no part.
    fn boundary(&self) -> String {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        self.boundary_from(seed)
    }

    fn boundary_from(&self, seed: u128) -> String {
        (0u32..)
            .map(|n| format!("grenat-{seed:x}-{n}"))
            .find(|b| !self.parts.iter().any(|(_, _, content)| contains(content, b.as_bytes())))
            .expect("a boundary")
    }
}

/// A name in a quoted header parameter: quotes and line breaks escaped.
fn quoted(name: &str) -> String {
    name.replace('"', "%22").replace('\r', "%0D").replace('\n', "%0A")
}

/// A header value on one line: control characters (CR, LF…) left out, so
/// that no value can start another header or the part's body.
fn one_line(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_then_a_file_between_boundaries() {
        let mut form = Form::default();
        form.text("model", "whisper-1");
        form.text("timestamp_granularities[]", "segment");
        form.file("file", "audio.mp3", "audio/mpeg", vec![0, 159, 255]);
        let (content_type, body) = form.finish();
        let boundary = content_type.strip_prefix("multipart/form-data; boundary=").unwrap().to_string();
        let mut expected = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"timestamp_granularities[]\"\r\n\r\nsegment\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.mp3\"\r\nContent-Type: audio/mpeg\r\n\r\n"
        )
        .into_bytes();
        expected.extend_from_slice(&[0, 159, 255]);
        expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        assert_eq!(body, expected);
    }

    #[test]
    fn the_boundary_is_in_no_part() {
        let mut form = Form::default();
        assert_eq!(form.boundary_from(31), "grenat-1f-0");
        form.text("a", "grenat-1f-0 and grenat-1f-1");
        assert_eq!(form.boundary_from(31), "grenat-1f-2");
        assert_eq!(quoted("a\"b\r\n"), "a%22b%0D%0A");
    }

    #[test]
    fn a_media_type_adds_no_header_line() {
        let mut form = Form::default();
        form.file("file", "a.mp3", "audio/mpeg; x\r\nX-Injected: 1\r\n\r\nINJECTED", vec![1]);
        let (_, body) = form.finish();
        let body = String::from_utf8(body).unwrap();
        assert!(!body.contains("\r\nX-Injected"), "{body:?}");
        assert!(body.contains("\r\nContent-Type: audio/mpeg; xX-Injected: 1INJECTED\r\n\r\n\u{1}\r\n"), "{body:?}");
    }
}
