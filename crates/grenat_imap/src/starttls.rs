//! The clear-text start of an `imap://` connection: the greeting,
//! `CAPABILITY`, then `STARTTLS` (RFC 9051 and RFC 3501, section 6.2.1).
//! Nothing else is ever sent in clear text: a server that does not offer
//! STARTTLS is refused before any credential exists on the wire.
//!
//! Lines are read one byte at a time, so that nothing after the server's
//! `OK` is read ahead: what an attacker injects there stays in the socket
//! and breaks the TLS handshake instead of being taken for an answer. The
//! capabilities heard here are forgotten; the caller asks again over TLS.

use std::io::{Read, Write};

use crate::error::{Error, ErrorKind, Result};

/// A server line longer than this, before TLS, is no IMAP.
const MAX_LINE: usize = 8 * 1024;

/// Reads the greeting, checks that the server offers STARTTLS and asks for
/// it; once this returns, the next bytes on `stream` are the TLS handshake.
pub(crate) fn negotiate(stream: &mut (impl Read + Write), host: &str) -> Result<()> {
    let greeting = read_line(stream)?;
    if greeting.starts_with("* PREAUTH") {
        return Err(Error::new(ErrorKind::Insecure, format!("{host} logs us in before TLS (PREAUTH): refused")));
    }
    if !greeting.starts_with("* OK") {
        return Err(Error::new(ErrorKind::Refused, format!("{host} does not welcome us: {greeting}")));
    }
    let lines = command(stream, "s1", "CAPABILITY")?;
    let offered = lines
        .iter()
        .filter_map(|line| line.strip_prefix("* CAPABILITY "))
        .any(|caps| caps.split(' ').any(|c| c.eq_ignore_ascii_case("STARTTLS")));
    if !offered {
        return Err(Error::new(
            ErrorKind::Insecure,
            format!("{host} offers no STARTTLS: credentials are never sent in clear text"),
        ));
    }
    command(stream, "s2", "STARTTLS")?;
    Ok(())
}

/// Sends `tag name`, and reads up to the tagged answer, which must be `OK`;
/// the untagged lines before it.
fn command(stream: &mut (impl Read + Write), tag: &str, name: &str) -> Result<Vec<String>> {
    stream
        .write_all(format!("{tag} {name}\r\n").as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|e| Error::io(name, &e))?;
    let mut untagged = Vec::new();
    loop {
        let line = read_line(stream)?;
        let Some(status) = line.strip_prefix(tag).and_then(|rest| rest.strip_prefix(' ')) else {
            untagged.push(line);
            continue;
        };
        if status.get(..2).is_some_and(|ok| ok.eq_ignore_ascii_case("OK")) {
            return Ok(untagged);
        }
        return Err(Error::new(ErrorKind::Refused, format!("{name}: {status}")));
    }
}

/// One line, without its CRLF.
fn read_line(stream: &mut impl Read) -> Result<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() <= MAX_LINE {
        match stream.read(&mut byte) {
            Ok(0) => return Err(Error::new(ErrorKind::Connect, "the server closed the connection before TLS")),
            Ok(_) if byte[0] == b'\n' => {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(String::from_utf8_lossy(&line).into_owned());
            }
            Ok(_) => line.push(byte[0]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Error::io("reading the server's answer", &e)),
        }
    }
    Err(Error::new(ErrorKind::Refused, "the server sent a line too long before TLS"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A server that has already said `script`; what the client wrote.
    struct Wire {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Wire {
        fn new(script: &str) -> Wire {
            Wire { input: Cursor::new(script.as_bytes().to_vec()), output: Vec::new() }
        }
    }

    impl Read for Wire {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Wire {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stops_right_after_the_ok() {
        let mut wire = Wire::new(
            "* OK ready\r\n* CAPABILITY IMAP4rev1 STARTTLS LOGINDISABLED\r\ns1 OK done\r\ns2 OK go\r\n* injected\r\n",
        );
        negotiate(&mut wire, "h").unwrap();
        assert_eq!(String::from_utf8(wire.output).unwrap(), "s1 CAPABILITY\r\ns2 STARTTLS\r\n");
        // what follows the OK is left for the TLS handshake (which it breaks)
        let mut rest = String::new();
        wire.input.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "* injected\r\n");
    }

    #[test]
    fn no_starttls_no_credentials() {
        let mut wire = Wire::new("* OK ready\r\n* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\ns1 OK done\r\n");
        let e = negotiate(&mut wire, "mail.acme.com").unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Insecure);
        assert!(e.message().contains("mail.acme.com offers no STARTTLS"), "{e}");
        assert_eq!(String::from_utf8(wire.output).unwrap(), "s1 CAPABILITY\r\n");
    }

    #[test]
    fn refusals() {
        let e = negotiate(&mut Wire::new("* PREAUTH welcome\r\n"), "h").unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Insecure);
        let e = negotiate(&mut Wire::new("* BYE busy\r\n"), "h").unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Refused);
        let e = negotiate(&mut Wire::new("* OK\r\n* CAPABILITY STARTTLS\r\ns1 OK\r\ns2 BAD no\r\n"), "h").unwrap_err();
        assert_eq!((e.kind(), e.message()), (ErrorKind::Refused, "STARTTLS: BAD no"));
        let e = negotiate(&mut Wire::new("* OK\r\n"), "h").unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Connect);
        let e = negotiate(&mut Wire::new(&"x".repeat(MAX_LINE + 10)), "h").unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Refused);
    }
}
