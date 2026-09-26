//! A SOCKS5 proxy for the tests (CONNECT only; no authentication, or a
//! user name and password), recording where it was asked to connect.

use std::net::TcpListener as StdListener;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub struct Socks {
    pub port: u16,
    /// `host:port` of every CONNECT, in order.
    pub targets: Arc<Mutex<Vec<String>>>,
}

impl Socks {
    /// A proxy requiring `credentials` (user, password), or none.
    pub fn start(credentials: Option<(&str, &str)>) -> Socks {
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let targets = Arc::new(Mutex::new(Vec::new()));
        let recorded = targets.clone();
        let credentials = credentials.map(|(u, p)| (u.to_string(), p.to_string()));
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            runtime.block_on(async move {
                let listener = TcpListener::from_std(listener).unwrap();
                while let Ok((client, _)) = listener.accept().await {
                    let (credentials, recorded) = (credentials.clone(), recorded.clone());
                    tokio::spawn(async move {
                        let _ = serve(client, credentials, recorded).await;
                    });
                }
            });
        });
        Socks { port, targets }
    }

    pub fn targets(&self) -> Vec<String> {
        self.targets.lock().unwrap().clone()
    }
}

async fn serve(
    mut client: TcpStream,
    credentials: Option<(String, String)>,
    targets: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    let mut methods = vec![0u8; head[1] as usize];
    client.read_exact(&mut methods).await?;
    let wanted = if credentials.is_some() { 2 } else { 0 };
    if head[0] != 5 || !methods.contains(&wanted) {
        client.write_all(&[5, 0xff]).await?;
        return Ok(());
    }
    client.write_all(&[5, wanted]).await?;
    if let Some((user, password)) = credentials {
        let given_user = read_field(&mut client, true).await?;
        let given_password = read_field(&mut client, false).await?;
        let accepted = given_user == user.as_bytes() && given_password == password.as_bytes();
        client.write_all(&[1, if accepted { 0 } else { 1 }]).await?;
        if !accepted {
            return Ok(());
        }
    }
    let mut request = [0u8; 4];
    client.read_exact(&mut request).await?;
    let host = match request[3] {
        1 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).await?;
            std::net::Ipv4Addr::from(ip).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        _ => return Ok(()),
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    let target = format!("{host}:{}", u16::from_be_bytes(port));
    targets.lock().unwrap().push(target.clone());
    match TcpStream::connect(&target).await {
        Ok(mut server) => {
            client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            tokio::io::copy_bidirectional(&mut client, &mut server).await?;
        }
        Err(_) => client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?,
    }
    Ok(())
}

/// A length-prefixed field of the RFC 1929 request (after its version byte
/// when `first`).
async fn read_field(client: &mut TcpStream, first: bool) -> std::io::Result<Vec<u8>> {
    if first {
        let mut version = [0u8; 1];
        client.read_exact(&mut version).await?;
    }
    let mut len = [0u8; 1];
    client.read_exact(&mut len).await?;
    let mut field = vec![0u8; len[0] as usize];
    client.read_exact(&mut field).await?;
    Ok(field)
}
