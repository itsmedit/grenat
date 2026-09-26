//! The TCP stream an SSH connection runs on: straight to the server, or
//! through a SOCKS5 proxy.

use tokio::net::TcpStream;

use crate::error::{Error, ErrorKind};
use crate::options::target;
use crate::socks5::{self, Proxy};

/// A stream reaching `host:port`.
pub async fn open(host: &str, port: u16, proxy: Option<&Proxy>) -> Result<TcpStream, Error> {
    let stream = match proxy {
        None => TcpStream::connect((host, port))
            .await
            .map_err(|e| Error::new(ErrorKind::Connect, format!("cannot connect to {}: {e}", target(host, port))))?,
        Some(proxy) => {
            let mut stream = TcpStream::connect((proxy.host.as_str(), proxy.port)).await.map_err(|e| {
                Error::new(ErrorKind::Proxy, format!("cannot reach the proxy {}: {e}", proxy.address()))
            })?;
            socks5::handshake(&mut stream, proxy, host, port).await?;
            stream
        }
    };
    stream.set_nodelay(true).ok();
    Ok(stream)
}
