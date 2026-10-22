//! A whole response, written out: its status line, its headers (a `Date`,
//! its length, whether the connection stays open) and its body.

use crate::calendar::civil;
use crate::status::reason;

/// What the response answers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Answering {
    pub http11: bool,
    /// The connection stays open after it.
    pub keep_alive: bool,
    /// A `HEAD` request: the head only.
    pub head_only: bool,
}

/// The bytes of a response; `now` in seconds since the epoch.
pub fn whole(
    status: u16,
    content_type: &str,
    headers: &[(String, String)],
    body: &[u8],
    to: Answering,
    now: i64,
) -> Vec<u8> {
    let mut head = format!("HTTP/1.{} {status} {}\r\n", u8::from(to.http11), reason(status));
    // the framing is the server's: a header of the program never overrides it
    let framing = ["content-length", "transfer-encoding", "connection"];
    let given = |name: &str| headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name));
    if !given("Date") {
        head.push_str(&format!("Date: {}\r\n", http_date(now)));
    }
    let all =
        std::iter::once(("Content-Type", content_type)).chain(headers.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    for (name, value) in all {
        if framing.iter().any(|f| name.eq_ignore_ascii_case(f)) || !valid_name(name) || !valid_value(value) {
            continue;
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    // no body, nor its length, for 1xx, 204 and 304 (RFC 9110 §8.6)
    let bodiless = status < 200 || status == 204 || status == 304;
    if !bodiless {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    match (to.http11, to.keep_alive) {
        (true, false) => head.push_str("Connection: close\r\n"),
        (false, true) => head.push_str("Connection: keep-alive\r\n"),
        _ => {}
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    if !bodiless && !to.head_only {
        bytes.extend_from_slice(body);
    }
    bytes
}

/// A header name: an HTTP token.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// A header value that cannot end the line it is on.
fn valid_value(value: &str) -> bool {
    !value.contains(['\r', '\n', '\0'])
}

/// `t` as an IMF-fixdate: `Sun, 06 Nov 1994 08:49:37 GMT`.
pub fn http_date(t: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (year, month, day, hour, minute, weekday) = civil(t);
    let second = t.rem_euclid(60);
    format!(
        "{}, {day:02} {} {year} {hour:02}:{minute:02}:{second:02} GMT",
        DAYS[weekday as usize],
        MONTHS[month as usize - 1]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEPT: Answering = Answering { http11: true, keep_alive: true, head_only: false };

    #[test]
    fn a_response_says_its_length_and_its_date() {
        let headers = [
            ("X-Id".to_string(), "7".to_string()),
            ("Bad".to_string(), "a\r\nSet-Cookie: x".to_string()),
            ("Content-Length".to_string(), "99".to_string()),
            ("Connection".to_string(), "upgrade".to_string()),
        ];
        let bytes = whole(201, "text/plain", &headers, b"hi", KEPT, 784_111_777);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "HTTP/1.1 201 Created\r\nDate: Sun, 06 Nov 1994 08:49:37 GMT\r\nContent-Type: text/plain\r\nX-Id: 7\r\nContent-Length: 2\r\n\r\nhi"
        );
    }

    #[test]
    fn the_connection_and_the_body_follow_the_request() {
        let text = |status, to| String::from_utf8(whole(status, "text/plain", &[], b"body", to, 0)).unwrap();
        let closed = text(200, Answering { keep_alive: false, ..KEPT });
        assert!(closed.contains("Connection: close\r\n") && closed.ends_with("body"), "{closed}");
        let old = text(200, Answering { http11: false, keep_alive: true, head_only: false });
        assert!(old.starts_with("HTTP/1.0 200 OK\r\n") && old.contains("Connection: keep-alive\r\n"), "{old}");
        let head = text(200, Answering { head_only: true, ..KEPT });
        assert!(head.contains("Content-Length: 4\r\n") && head.ends_with("\r\n\r\n"), "{head}");
        let empty = text(204, KEPT);
        assert!(!empty.contains("Content-Length") && empty.ends_with("\r\n\r\n"), "{empty}");
        assert!(text(200, KEPT).contains("Date: Thu, 01 Jan 1970 00:00:00 GMT\r\n"));
    }
}
