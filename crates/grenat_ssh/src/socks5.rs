//! The client side of a SOCKS5 proxy (RFC 1928), with username and password
//! authentication (RFC 1929): the proxy URL, and the handshake asking the
//! proxy to connect to the SSH server. The server's name travels to the
//! proxy, which resolves it. Messages name the proxy by `host:port` only,
//! never with its credentials.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Error, ErrorKind};
use crate::options::target;

const VERSION: u8 = 5;
const NO_AUTH: u8 = 0;
const USER_PASSWORD: u8 = 2;
const NO_ACCEPTABLE: u8 = 0xff;
const CONNECT: u8 = 1;
const IPV4: u8 = 1;
const DOMAIN: u8 = 3;
const IPV6: u8 = 4;

#[derive(Clone, PartialEq, Eq)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    credentials: Option<(String, String)>,
}

impl Proxy {
    /// `socks5://[user:password@]host[:port]` (or `socks5h://`); port 1080
    /// by default; user and password percent-decoded.
    pub fn parse(url: &str) -> Result<Proxy, Error> {
        let bad = || {
            Error::new(ErrorKind::Proxy, "the proxy URL is not valid (expected socks5://[user:password@]host[:port])")
        };
        let rest = url.strip_prefix("socks5://").or_else(|| url.strip_prefix("socks5h://")).ok_or_else(bad)?;
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        let (userinfo, address) = match rest.rsplit_once('@') {
            Some((userinfo, address)) => (Some(userinfo), address),
            None => (None, rest),
        };
        let credentials = match userinfo {
            None => None,
            Some(userinfo) => {
                let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
                let (user, password) =
                    (percent_decode(user).ok_or_else(bad)?, percent_decode(password).ok_or_else(bad)?);
                if user.is_empty() || user.len() > 255 || password.len() > 255 {
                    return Err(bad());
                }
                Some((user, password))
            }
        };
        let (host, port) = split_host_port(address).ok_or_else(bad)?;
        if host.is_empty() || host.len() > 255 || address.contains(['/', '?', '#']) {
            return Err(bad());
        }
        Ok(Proxy { host, port, credentials })
    }

    /// `host:port`, for messages.
    pub fn address(&self) -> String {
        target(&self.host, self.port)
    }
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("credentials", &self.credentials.as_ref().map(|_| "<secret>"))
            .finish()
    }
}

/// `host`, `host:port`, `[v6]`, `[v6]:port`.
fn split_host_port(address: &str) -> Option<(String, u16)> {
    if let Some(bracketed) = address.strip_prefix('[') {
        let (host, rest) = bracketed.split_once(']')?;
        let port = match rest.strip_prefix(':') {
            Some(port) => port.parse().ok()?,
            None if rest.is_empty() => 1080,
            None => return None,
        };
        return Some((host.to_string(), port));
    }
    match address.split_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok().filter(|p| *p != 0)?)),
        None => Some((address.to_string(), 1080)),
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Asks the proxy at the other end of `stream` to connect to `host:port`;
/// afterwards the stream reaches that server.
pub async fn handshake<S>(stream: &mut S, proxy: &Proxy, host: &str, port: u16) -> Result<(), Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let at = proxy.address();
    let io = |e: std::io::Error| Error::new(ErrorKind::Proxy, format!("the proxy {at} broke the connection: {e}"));
    let method = if proxy.credentials.is_some() { USER_PASSWORD } else { NO_AUTH };
    stream.write_all(&[VERSION, 1, method]).await.map_err(io)?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).await.map_err(io)?;
    if choice[0] != VERSION {
        return Err(Error::new(ErrorKind::Proxy, format!("{at} is not a SOCKS5 proxy")));
    }
    match (choice[1], &proxy.credentials) {
        (NO_AUTH, None) => {}
        (USER_PASSWORD, Some((user, password))) => {
            let mut request = vec![1, user.len() as u8];
            request.extend_from_slice(user.as_bytes());
            request.push(password.len() as u8);
            request.extend_from_slice(password.as_bytes());
            stream.write_all(&request).await.map_err(io)?;
            let mut status = [0u8; 2];
            stream.read_exact(&mut status).await.map_err(io)?;
            if status[1] != 0 {
                return Err(Error::new(ErrorKind::Proxy, format!("the proxy {at} refused the user name or password")));
            }
        }
        (NO_ACCEPTABLE, Some(_)) => {
            return Err(Error::new(
                ErrorKind::Proxy,
                format!("the proxy {at} does not accept a user name and password"),
            ));
        }
        (NO_ACCEPTABLE, None) => {
            return Err(Error::new(ErrorKind::Proxy, format!("the proxy {at} requires authentication")));
        }
        _ => return Err(Error::new(ErrorKind::Proxy, format!("the proxy {at} chose an authentication not offered"))),
    }

    stream.write_all(&connect_request(host, port)?).await.map_err(io)?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await.map_err(io)?;
    if reply[1] != 0 {
        return Err(Error::new(
            ErrorKind::Proxy,
            format!("the proxy {at} cannot reach {}: {}", target(host, port), reply_text(reply[1])),
        ));
    }
    let bound = match reply[3] {
        IPV4 => 4,
        IPV6 => 16,
        DOMAIN => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await.map_err(io)?;
            len[0] as usize
        }
        _ => return Err(Error::new(ErrorKind::Proxy, format!("the proxy {at} sent an invalid reply"))),
    };
    let mut rest = vec![0u8; bound + 2];
    stream.read_exact(&mut rest).await.map_err(io)?;
    Ok(())
}

/// CONNECT to an IP address, or to a name the proxy resolves.
fn connect_request(host: &str, port: u16) -> Result<Vec<u8>, Error> {
    let mut request = vec![VERSION, CONNECT, 0];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            request.push(IPV4);
            request.extend_from_slice(&ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            request.push(IPV6);
            request.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            if host.len() > 255 {
                return Err(Error::new(ErrorKind::Proxy, "the host name is too long for a SOCKS5 proxy"));
            }
            request.push(DOMAIN);
            request.push(host.len() as u8);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    Ok(request)
}

fn reply_text(code: u8) -> &'static str {
    match code {
        1 => "general failure",
        2 => "not allowed by its rules",
        3 => "network unreachable",
        4 => "host unreachable",
        5 => "connection refused",
        6 => "TTL expired",
        7 => "command not supported",
        8 => "address type not supported",
        _ => "unknown error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn run<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    #[test]
    fn urls() {
        let p = Proxy::parse("socks5://proxy.local:9050").unwrap();
        assert_eq!((p.host.as_str(), p.port, p.credentials.clone()), ("proxy.local", 9050, None));
        assert_eq!(Proxy::parse("socks5h://proxy.local/").unwrap().port, 1080);
        let p = Proxy::parse("socks5://bob:p%40ss%3Aword@10.0.0.1:1080").unwrap();
        assert_eq!(p.credentials, Some(("bob".into(), "p@ss:word".into())));
        assert_eq!(p.address(), "10.0.0.1:1080");
        let shown = format!("{p:?}");
        assert!(!shown.contains("p@ss") && shown.contains("<secret>"), "{shown}");
        let p = Proxy::parse("socks5://[::1]:1081").unwrap();
        assert_eq!((p.host.as_str(), p.port, p.address().as_str()), ("::1", 1081, "[::1]:1081"));
        for bad in [
            "http://proxy:80",
            "socks5://",
            "socks5://proxy:0",
            "socks5://proxy:x",
            "socks5://:pass@proxy",
            "socks5://u:%zz@proxy",
            "socks5://proxy/path",
            "socks5://[::1",
        ] {
            let e = Proxy::parse(bad).unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Proxy, "{bad}");
        }
        let e = Proxy::parse("ftp://bob:s3cret@proxy").unwrap_err();
        assert!(!e.message().contains("s3cret"), "{e}");
    }

    #[test]
    fn connect_requests() {
        assert_eq!(connect_request("10.0.0.1", 22).unwrap(), [5, 1, 0, 1, 10, 0, 0, 1, 0, 22]);
        assert_eq!(connect_request("ab.c", 2222).unwrap(), [5, 1, 0, 3, 4, b'a', b'b', b'.', b'c', 8, 174]);
        assert_eq!(connect_request("::1", 22).unwrap()[3], IPV6);
        assert!(connect_request(&"a".repeat(256), 22).is_err());
    }

    /// Plays the proxy's side: checks what the client sends, answers `replies`.
    async fn exchange(proxy: &Proxy, expected: Vec<u8>, replies: Vec<u8>) -> Result<(), Error> {
        let (mut client, mut server) = duplex(1024);
        let peer = tokio::spawn(async move {
            server.write_all(&replies).await.unwrap();
            let mut got = vec![0u8; expected.len()];
            server.read_exact(&mut got).await.unwrap();
            assert_eq!(got, expected);
            server
        });
        let result = handshake(&mut client, proxy, "ssh.example", 22).await;
        drop(client);
        let _ = peer.await;
        result
    }

    fn target() -> Vec<u8> {
        let mut t = vec![5, 1, 0, 3, 11];
        t.extend_from_slice(b"ssh.example");
        t.extend_from_slice(&[0, 22]);
        t
    }

    #[test]
    fn handshakes() {
        run(async {
            let open = Proxy::parse("socks5://p:1").unwrap();
            let mut expected = vec![5, 1, 0];
            expected.extend(target());
            let ok = vec![5, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0];
            assert!(exchange(&open, expected.clone(), ok).await.is_ok());

            let refused = vec![5, 0, 5, 5, 0, 1, 0, 0, 0, 0, 0, 0];
            let e = exchange(&open, expected, refused).await.unwrap_err();
            assert!(e.message().contains("connection refused"), "{e}");

            let e = exchange(&open, vec![5, 1, 0], vec![5, 0xff]).await.unwrap_err();
            assert!(e.message().contains("requires authentication"), "{e}");

            let closed = Proxy::parse("socks5://bob:pw@p:1").unwrap();
            let mut expected = vec![5, 1, 2, 1, 3, b'b', b'o', b'b', 2, b'p', b'w'];
            expected.extend(target());
            let ok = vec![5, 2, 1, 0, 5, 0, 0, 3, 1, b'x', 0, 0];
            assert!(exchange(&closed, expected, ok).await.is_ok());

            let e = exchange(&closed, vec![5, 1, 2, 1, 3, b'b', b'o', b'b', 2, b'p', b'w'], vec![5, 2, 1, 1])
                .await
                .unwrap_err();
            assert!(e.message().contains("refused the user name or password") && !e.message().contains("pw"), "{e}");

            let e = exchange(&open, vec![5, 1, 0], vec![4, 0]).await.unwrap_err();
            assert!(e.message().contains("not a SOCKS5 proxy"), "{e}");
        });
    }
}
