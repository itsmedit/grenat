//! What the SSH library asks the client during a connection: whether to
//! trust the server's host key. The answer comes from [`host_key::verify`];
//! a refusal ends the handshake with that error, before any credential is
//! sent.

use std::sync::{Arc, Mutex};

use russh::keys::PublicKeyOrCertificate;

use crate::error::{Error, ErrorKind};
use crate::host_key::{self, HostKey};
use crate::options::KnownHosts;

pub struct Client {
    policy: KnownHosts,
    host: String,
    port: u16,
    /// The key accepted, for the session to report.
    accepted: Arc<Mutex<Option<HostKey>>>,
}

impl Client {
    pub fn new(policy: KnownHosts, host: &str, port: u16) -> (Client, Arc<Mutex<Option<HostKey>>>) {
        let accepted = Arc::new(Mutex::new(None));
        (Client { policy, host: host.to_string(), port, accepted: accepted.clone() }, accepted)
    }
}

impl russh::client::Handler for Client {
    type Error = Error;

    async fn check_server_key(&mut self, offered: &PublicKeyOrCertificate) -> Result<bool, Error> {
        let key = match offered {
            PublicKeyOrCertificate::PublicKey { key, .. } => key,
            PublicKeyOrCertificate::Certificate(_) => {
                return Err(Error::new(
                    ErrorKind::HostKey,
                    format!("{}:{} offered a host certificate, which is not supported", self.host, self.port),
                ));
            }
        };
        let accepted = host_key::verify(&self.policy, &self.host, self.port, key)?;
        *self.accepted.lock().unwrap_or_else(|e| e.into_inner()) = Some(accepted);
        Ok(true)
    }
}
