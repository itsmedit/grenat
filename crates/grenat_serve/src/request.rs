//! An HTTP/1.x request, read from a connection: its head (request line and
//! headers) and its body (`Content-Length` or chunked), within limits —
//! a client cannot make the server buffer what it likes, nor smuggle a
//! second request inside the first.
//!
//! Time is not counted here: the connection sets its socket's timeouts.

use std::io::{BufRead, ErrorKind, Write};

/// How much a request may hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sizes {
    /// The request line and the headers, together.
    pub head: usize,
    /// The number of headers.
    pub headers: usize,
    /// The body, once decoded.
    pub body: usize,
}

impl Default for Sizes {
    fn default() -> Self {
        Sizes { head: 64 * 1024, headers: 100, body: 32 * 1024 * 1024 }
    }
}

/// A request read.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: String,
    /// As the client wrote it: the path and the query.
    pub target: String,
    /// `HTTP/1.1` (true) or `HTTP/1.0`.
    pub http11: bool,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Whether the client asks for the connection to stay open after it.
    pub fn keep_alive(&self) -> bool {
        let tokens = self.header("Connection").unwrap_or_default().to_ascii_lowercase();
        let has = |token: &str| tokens.split(',').any(|t| t.trim() == token);
        if self.http11 { !has("close") } else { has("keep-alive") }
    }
}

/// Why no request could be read.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// The connection ended (or timed out) before a request began: nothing
    /// to answer.
    Gone,
    /// A request the server refuses, answered with this status before the
    /// connection is closed.
    Refused(u16),
}

impl From<std::io::Error> for Failure {
    fn from(_: std::io::Error) -> Self {
        Failure::Gone
    }
}

/// Reads the next request, its head then its body.
#[cfg(test)]
pub fn read(reader: &mut impl BufRead, interim: &mut impl Write, sizes: Sizes) -> Result<Request, Failure> {
    let (mut request, framing) = read_head(reader, interim, sizes)?;
    request.body = read_body(reader, &framing, sizes)?;
    Ok(request)
}

/// Reads the head of the next request (its body left empty) and how its
/// body is delimited. `interim` receives the `100 Continue` a client waits
/// for before sending its body.
pub fn read_head(
    reader: &mut impl BufRead,
    interim: &mut impl Write,
    sizes: Sizes,
) -> Result<(Request, Framing), Failure> {
    let mut budget = sizes.head;
    // a line or two left empty between requests is tolerated (RFC 9112 §2.2)
    let line = loop {
        let line = read_line(reader, &mut budget)?.ok_or(Failure::Gone)?;
        if !line.is_empty() {
            break line;
        }
    };
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Failure::Refused(400));
    };
    let http11 = match version {
        "HTTP/1.1" => true,
        "HTTP/1.0" => false,
        v if v.starts_with("HTTP/") => return Err(Failure::Refused(505)),
        _ => return Err(Failure::Refused(400)),
    };
    if !is_token(method) || target.is_empty() || target.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(Failure::Refused(400));
    }
    let mut headers = Vec::new();
    loop {
        let line = read_line(reader, &mut budget)?.ok_or(Failure::Refused(400))?;
        if line.is_empty() {
            break;
        }
        if headers.len() == sizes.headers {
            return Err(Failure::Refused(431));
        }
        // no folded line (obsolete, RFC 9112 §5.2), no space before the colon
        let Some((name, value)) = line.split_once(':') else { return Err(Failure::Refused(400)) };
        if !is_token(name) {
            return Err(Failure::Refused(400));
        }
        headers.push((name.to_string(), value.trim_matches([' ', '\t']).to_string()));
    }
    let request = Request { method: method.to_string(), target: target.to_string(), http11, headers, body: Vec::new() };
    let framing = framing(&request)?;
    if framing != Framing::None {
        expect_continue(&request, interim)?;
    }
    if let Framing::Length(length) = framing
        && length > sizes.body
    {
        return Err(Failure::Refused(413));
    }
    Ok((request, framing))
}

/// Reads the body the head announced.
pub fn read_body(reader: &mut impl BufRead, framing: &Framing, sizes: Sizes) -> Result<Vec<u8>, Failure> {
    match *framing {
        Framing::None => Ok(Vec::new()),
        Framing::Length(length) => read_exact(reader, length),
        Framing::Chunked => read_chunked(reader, sizes),
    }
}

/// How the body is delimited.
#[derive(Debug, PartialEq)]
pub enum Framing {
    None,
    Length(usize),
    Chunked,
}

fn framing(request: &Request) -> Result<Framing, Failure> {
    let all = |name: &'static str| -> Vec<&String> {
        request.headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v).collect()
    };
    let encodings = all("Transfer-Encoding");
    let lengths = all("Content-Length");
    if !encodings.is_empty() {
        // both, or an encoding other than chunked alone: a request a proxy
        // in front may read otherwise (request smuggling)
        if !lengths.is_empty() || !request.http11 {
            return Err(Failure::Refused(400));
        }
        let joined = encodings.iter().map(|v| v.as_str()).collect::<Vec<_>>().join(",");
        return match joined.trim().eq_ignore_ascii_case("chunked") {
            true => Ok(Framing::Chunked),
            false => Err(Failure::Refused(501)),
        };
    }
    let mut length = None;
    for value in lengths.iter().flat_map(|v| v.split(',')) {
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Failure::Refused(400));
        }
        let parsed = value.parse::<usize>().map_err(|_| Failure::Refused(413))?;
        if length.is_some_and(|l| l != parsed) {
            return Err(Failure::Refused(400));
        }
        length = Some(parsed);
    }
    Ok(match length {
        None | Some(0) => Framing::None,
        Some(n) => Framing::Length(n),
    })
}

/// Answers `Expect: 100-continue`; any other expectation is refused.
fn expect_continue(request: &Request, interim: &mut impl Write) -> Result<(), Failure> {
    match request.header("Expect") {
        None => Ok(()),
        Some(e) if e.eq_ignore_ascii_case("100-continue") => {
            if request.http11 {
                interim.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
                interim.flush()?;
            }
            Ok(())
        }
        Some(_) => Err(Failure::Refused(417)),
    }
}

/// A line ending in `\n` (an `\r` before it dropped), counted against
/// `budget`; `None` when the connection ends before any byte.
fn read_line(reader: &mut impl BufRead, budget: &mut usize) -> Result<Option<String>, Failure> {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) if line.is_empty() => return Ok(None),
            Err(_) => return Err(Failure::Refused(408)),
        };
        if available.is_empty() {
            return if line.is_empty() { Ok(None) } else { Err(Failure::Refused(400)) };
        }
        let (taken, ended) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if taken > *budget {
            return Err(Failure::Refused(431));
        }
        *budget -= taken;
        line.extend_from_slice(&available[..taken]);
        reader.consume(taken);
        if ended {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            // a bare `\r` or a control byte inside the head: refused
            if line.iter().any(|&b| b == b'\r' || b == 0) {
                return Err(Failure::Refused(400));
            }
            return String::from_utf8(line).map(Some).map_err(|_| Failure::Refused(400));
        }
    }
}

fn read_exact(reader: &mut impl BufRead, length: usize) -> Result<Vec<u8>, Failure> {
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|e| match e.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => Failure::Refused(408),
        _ => Failure::Refused(400),
    })?;
    Ok(body)
}

/// A chunked body, decoded; its trailers are read and dropped.
fn read_chunked(reader: &mut impl BufRead, sizes: Sizes) -> Result<Vec<u8>, Failure> {
    let mut body = Vec::new();
    let mut budget = sizes.head;
    loop {
        let line = read_line(reader, &mut budget)?.ok_or(Failure::Refused(400))?;
        let size = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size, 16).map_err(|_| Failure::Refused(400))?;
        if size == 0 {
            break;
        }
        if size > sizes.body - body.len() {
            return Err(Failure::Refused(413));
        }
        body.extend_from_slice(&read_exact(reader, size)?);
        if read_line(reader, &mut budget)?.as_deref() != Some("") {
            return Err(Failure::Refused(400));
        }
    }
    while !read_line(reader, &mut budget)?.ok_or(Failure::Refused(400))?.is_empty() {}
    Ok(body)
}

/// An HTTP token (RFC 9110 §5.6.2): a method, a header's name.
fn is_token(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> (Result<Request, Failure>, Vec<u8>) {
        let mut interim = Vec::new();
        let result = read(&mut &bytes[..], &mut interim, Sizes { head: 256, headers: 4, body: 16 });
        (result, interim)
    }

    #[test]
    fn a_request_is_read_with_its_body() {
        let (request, interim) =
            parse(b"POST /hook?x=1 HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\nX-Sig:  v \r\n\r\nhello");
        let request = request.unwrap();
        assert_eq!((request.method.as_str(), request.target.as_str(), request.http11), ("POST", "/hook?x=1", true));
        assert_eq!(request.header("x-sig"), Some("v"));
        assert_eq!(request.body, b"hello");
        assert!(request.keep_alive() && interim.is_empty());

        let (request, _) = parse(b"\r\nGET / HTTP/1.0\nConnection: Keep-Alive\n\n");
        let request = request.unwrap();
        assert!(!request.http11 && request.keep_alive() && request.body.is_empty());
        let (request, _) = parse(b"GET / HTTP/1.0\r\n\r\n");
        assert!(!request.unwrap().keep_alive());
        let (request, _) = parse(b"GET / HTTP/1.1\r\nConnection: keep-alive, close\r\n\r\n");
        assert!(!request.unwrap().keep_alive());
    }

    #[test]
    fn a_chunked_body_is_decoded_and_100_continue_answered() {
        let bytes = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nExpect: 100-continue\r\n\r\n3;x=y\r\nabc\r\n2\r\nde\r\n0\r\nT: 1\r\n\r\nGET";
        let (request, interim) = parse(bytes);
        assert_eq!(request.unwrap().body, b"abcde");
        assert_eq!(interim, b"HTTP/1.1 100 Continue\r\n\r\n");
    }

    #[test]
    fn what_could_smuggle_or_exhaust_is_refused() {
        let refused = |bytes: &[u8]| parse(bytes).0.unwrap_err();
        let both = b"POST / HTTP/1.1\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        assert_eq!(refused(both), Failure::Refused(400));
        assert_eq!(
            refused(b"POST / HTTP/1.1\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd"),
            Failure::Refused(400)
        );
        assert_eq!(refused(b"POST / HTTP/1.1\r\nContent-Length: +3\r\n\r\nabc"), Failure::Refused(400));
        assert_eq!(refused(b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n"), Failure::Refused(501));
        assert_eq!(refused(b"POST / HTTP/1.1\r\nContent-Length: 17\r\n\r\n"), Failure::Refused(413));
        assert_eq!(refused(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n11\r\n"), Failure::Refused(413));
        assert_eq!(refused(b"GET / HTTP/1.1\r\nA: 1\r\n folded\r\n\r\n"), Failure::Refused(400));
        assert_eq!(refused(b"GET / HTTP/1.1\r\nA : 1\r\n\r\n"), Failure::Refused(400));
        assert_eq!(refused(b"GET / HTTP/1.1\r\nA: 1\rB: 2\r\n\r\n"), Failure::Refused(400));
        assert_eq!(refused(b"GET / HTTP/1.1\r\nA: 1\r\nB: 1\r\nC: 1\r\nD: 1\r\nE: 1\r\n\r\n"), Failure::Refused(431));
        assert_eq!(refused(format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(300)).as_bytes()), Failure::Refused(431));
        assert_eq!(refused(b"GET / HTTP/2.0\r\n\r\n"), Failure::Refused(505));
        assert_eq!(refused(b"GET /\r\n\r\n"), Failure::Refused(400));
        assert_eq!(refused(b"GET / HTTP/1.1\r\nExpect: magic\r\nContent-Length: 1\r\n\r\na"), Failure::Refused(417));
        assert_eq!(refused(b"GET / HTTP/1.1\r\nHost: a\r\n"), Failure::Refused(400));
        assert_eq!(refused(b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nab"), Failure::Refused(400));
        assert_eq!(refused(b""), Failure::Gone);
    }
}
