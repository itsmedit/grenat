//! The clear-text start of an `imap://` connection: the greeting,
//! `CAPABILITY`, then `STARTTLS` (RFC 9051 and RFC 3501, section 6.2.1).
//! Nothing else is ever sent in clear text: a server that does not offer
//! STARTTLS is refused before any credential exists on the wire.
//!
//! Lines are read one byte at a time, so that nothing after the server's
//! `OK` is read ahead: what an attacker injects there stays in the socket
//! and breaks the TLS handshake instead of being taken for an answer. The
//! capabilities heard here are forgotten; the caller asks again over TLS.
//!
//! Anyone on the network path can write here, so the exchange is bounded:
//! each line ([`MAX_LINE`]), the lines before an answer ([`MAX_UNTAGGED`]),
//! and the whole exchange (the connection's timeout, checked between reads).

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::error::{Error, ErrorKind, Result};

/// A server line longer than this, before TLS, is no IMAP.
const MAX_LINE: usize = 8 * 1024;
/// More untagged lines than this before an answer, before TLS, is no IMAP
/// either (a server sends one: its capabilities).
const MAX_UNTAGGED: usize = 64;

/// Reads the greeting, checks that the server offers STARTTLS and asks for
/// it, all within `timeout`; once this returns, the next bytes on `stream`
/// are the TLS handshake.
pub(crate) fn negotiate(stream: &mut (impl Read + Write), host: &str, timeout: Duration) -> Result<()> {
    let mut clear = Clear { stream, deadline: Instant::now() + timeout, timeout };
    let greeting = clear.read_line()?;
    if greeting.starts_with("* PREAUTH") {
        return Err(Error::new(ErrorKind::Insecure, format!("{host} logs us in before TLS (PREAUTH): refused")));
    }
    if !greeting.starts_with("* OK") {
        return Err(Error::new(ErrorKind::Refused, format!("{host} does not welcome us: {greeting}")));
    }
    let lines = clear.command("s1", "CAPABILITY")?;
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
    clear.command("s2", "STARTTLS")?;
    Ok(())
}

/// The connection before TLS, and when the exchange must be over.
struct Clear<'a, S> {
    stream: &'a mut S,
    deadline: Instant,
    timeout: Duration,
}

impl<S: Read + Write> Clear<'_, S> {
    /// Sends `tag name`, and reads up to the tagged answer, which must be
    /// `OK`; the untagged lines before it.
    fn command(&mut self, tag: &str, name: &str) -> Result<Vec<String>> {
        self.stream
            .write_all(format!("{tag} {name}\r\n").as_bytes())
            .and_then(|()| self.stream.flush())
            .map_err(|e| Error::io(name, &e))?;
        let mut untagged = Vec::new();
        loop {
            let line = self.read_line()?;
            let Some(status) = line.strip_prefix(tag).and_then(|rest| rest.strip_prefix(' ')) else {
                if untagged.len() == MAX_UNTAGGED {
                    let message = format!("{name}: the server sent over {MAX_UNTAGGED} lines before TLS");
                    return Err(Error::new(ErrorKind::Refused, message));
                }
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
    fn read_line(&mut self) -> Result<String> {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while line.len() <= MAX_LINE {
            if Instant::now() > self.deadline {
                let message = format!("the server took over {:?} to start TLS", self.timeout);
                return Err(Error::new(ErrorKind::Timeout, message));
            }
            match self.stream.read(&mut byte) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const MINUTE: Duration = Duration::from_secs(60);

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
        negotiate(&mut wire, "h", MINUTE).unwrap();
        assert_eq!(String::from_utf8(wire.output).unwrap(), "s1 CAPABILITY\r\ns2 STARTTLS\r\n");
        // what follows the OK is left for the TLS handshake (which it breaks)
        let mut rest = String::new();
        wire.input.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "* injected\r\n");
    }

    #[test]
    fn no_starttls_no_credentials() {
        let mut wire = Wire::new("* OK ready\r\n* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\ns1 OK done\r\n");
        let e = negotiate(&mut wire, "mail.acme.com", MINUTE).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Insecure);
        assert!(e.message().contains("mail.acme.com offers no STARTTLS"), "{e}");
        assert_eq!(String::from_utf8(wire.output).unwrap(), "s1 CAPABILITY\r\n");
    }

    #[test]
    fn refusals() {
        let e = negotiate(&mut Wire::new("* PREAUTH welcome\r\n"), "h", MINUTE).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Insecure);
        let e = negotiate(&mut Wire::new("* BYE busy\r\n"), "h", MINUTE).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Refused);
        let e = negotiate(&mut Wire::new("* OK\r\n* CAPABILITY STARTTLS\r\ns1 OK\r\ns2 BAD no\r\n"), "h", MINUTE)
            .unwrap_err();
        assert_eq!((e.kind(), e.message()), (ErrorKind::Refused, "STARTTLS: BAD no"));
        let e = negotiate(&mut Wire::new("* OK\r\n"), "h", MINUTE).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Connect);
        let e = negotiate(&mut Wire::new(&"x".repeat(MAX_LINE + 10)), "h", MINUTE).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Refused);
    }

    #[test]
    fn a_flood_of_lines_before_tls_is_refused() {
        let flood = format!("* OK ready\r\n{}", "* AAAA\r\n".repeat(MAX_UNTAGGED + 1));
        let e = negotiate(&mut Wire::new(&flood), "h", MINUTE).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Refused, "{e}");
        assert_eq!(e.message(), "CAPABILITY: the server sent over 64 lines before TLS");
        // a few are fine
        let some = format!("* OK\r\n{}* CAPABILITY STARTTLS\r\ns1 OK\r\ns2 OK\r\n", "* hi\r\n".repeat(10));
        negotiate(&mut Wire::new(&some), "h", MINUTE).unwrap();
    }

    /// A server that answers one byte at a time, slowly.
    struct Slow(Wire);

    impl Read for Slow {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(Duration::from_millis(2));
            self.0.read(&mut buf[..1])
        }
    }

    impl Write for Slow {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_whole_exchange_has_a_deadline() {
        // each byte arrives in time, the exchange never ends in time
        let mut slow = Slow(Wire::new(&format!("* OK {}\r\n", "x".repeat(MAX_LINE))));
        let e = negotiate(&mut slow, "h", Duration::from_millis(100)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Timeout, "{e}");
        assert!(e.message().contains("to start TLS"), "{e}");
    }
}
