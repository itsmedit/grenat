//! The connection: TCP, then TLS (rustls, `ring`, Mozilla's roots) at
//! once or after STARTTLS — or, for a server on this machine that was
//! explicitly allowed to, clear text.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::error::{Error, ErrorKind, Result};

/// A connection to the server, protected or (on this machine) not.
pub(crate) enum Stream {
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
    Plain(TcpStream),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Tls(tls) => tls.read(buf),
            Stream::Plain(tcp) => tcp.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Tls(tls) => tls.write(buf),
            Stream::Plain(tcp) => tcp.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Tls(tls) => tls.flush(),
            Stream::Plain(tcp) => tcp.flush(),
        }
    }
}

/// Which certificates a server may present: those Mozilla trusts, and
/// any added (a test's own authority).
#[derive(Clone, Default)]
pub struct Trust {
    extra: Vec<CertificateDer<'static>>,
}

impl Trust {
    /// Also trusts the authority (or self-signed certificate) `der`.
    pub fn with(mut self, der: Vec<u8>) -> Trust {
        self.extra.push(CertificateDer::from(der));
        self
    }

    fn config(&self) -> Result<Arc<ClientConfig>> {
        let mut roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        for der in &self.extra {
            roots.add(der.clone()).map_err(|e| Error::new(ErrorKind::Tls, format!("a trusted certificate: {e}")))?;
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::new(ErrorKind::Tls, e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Arc::new(config))
    }
}

/// Opens a TCP connection to `host:port`, within `timeout` (to connect,
/// then for each read and write); with `loopback_only`, to an address of
/// this machine only, whatever the name resolves to.
pub(crate) fn connect(host: &str, port: u16, timeout: Duration, loopback_only: bool) -> Result<TcpStream> {
    let unreachable = |e: std::io::Error| Error::io(&format!("cannot reach {host}:{port}"), &e);
    let addresses = (host, port).to_socket_addrs().map_err(unreachable)?;
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no address");
    for address in addresses {
        if loopback_only && !address.ip().is_loopback() {
            return Err(Error::new(
                ErrorKind::Insecure,
                format!("{host} is {}, not this machine: IMAP without TLS is refused", address.ip()),
            ));
        }
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(timeout)).map_err(unreachable)?;
                tcp.set_write_timeout(Some(timeout)).map_err(unreachable)?;
                return Ok(tcp);
            }
            Err(e) => last = e,
        }
    }
    Err(unreachable(last))
}

/// TLS over `tcp`, the server's certificate checked against `host`. The
/// handshake completes here, so that a bad certificate is a TLS error.
pub(crate) fn secure(tcp: TcpStream, host: &str, trust: &Trust) -> Result<Stream> {
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| Error::new(ErrorKind::Url, format!("`{host}` is not a host name")))?;
    let connection =
        ClientConnection::new(trust.config()?, name).map_err(|e| Error::new(ErrorKind::Tls, e.to_string()))?;
    let mut tls = StreamOwned::new(connection, tcp);
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock).map_err(|e| match e.kind() {
            std::io::ErrorKind::InvalidData => Error::new(ErrorKind::Tls, format!("TLS with {host}: {e}")),
            _ => Error::io(&format!("TLS with {host}"), &e),
        })?;
    }
    Ok(Stream::Tls(Box::new(tls)))
}
