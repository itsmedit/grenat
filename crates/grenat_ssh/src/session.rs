//! An authenticated SSH connection, used through blocking calls: run
//! commands, open SFTP. Connecting checks the options, reads the private
//! key, opens the stream (maybe through the proxy), verifies the host key
//! and authenticates, all within the timeout.

use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};

use crate::auth::{self, Credential};
use crate::error::{Error, ErrorKind};
use crate::exec::{self, Output};
use crate::handler::Client;
use crate::host_key::HostKey;
use crate::options::{Auth, Options};
use crate::private_key;
use crate::quote;
use crate::runtime::{Runtime, within};
use crate::sftp::Sftp;
use crate::socks5::Proxy;
use crate::transport;

/// How often an idle connection checks that the server is still there
/// (while a call is running), and how many unanswered checks end it.
const KEEPALIVE: Duration = Duration::from_secs(15);
const KEEPALIVE_MAX: usize = 3;

pub struct Session {
    runtime: Arc<Runtime>,
    handle: Option<Handle<Client>>,
    host_key: HostKey,
    target: String,
    timeout: Option<Duration>,
}

impl Session {
    pub fn connect(options: Options) -> Result<Session, Error> {
        options.validate()?;
        let target = options.target();
        let proxy = options.proxy.as_deref().map(Proxy::parse).transpose()?;
        let credential = match &options.auth {
            Auth::Key { text, passphrase } => {
                Credential::Key(Box::new(private_key::decode(text, passphrase.as_deref())?))
            }
            Auth::Password(password) => Credential::Password(password),
        };
        let runtime = Arc::new(Runtime::new()?);
        let (handle, host_key) =
            runtime.block_on(within(options.timeout, || format!("connecting to {target}"), async {
                let stream = transport::open(&options.host, options.port, proxy.as_ref()).await?;
                let (handler, accepted) = Client::new(options.known_hosts.clone(), &options.host, options.port);
                let config = client::Config {
                    keepalive_interval: Some(KEEPALIVE),
                    keepalive_max: KEEPALIVE_MAX,
                    nodelay: true,
                    ..client::Config::default()
                };
                let mut handle = client::connect_stream(Arc::new(config), stream, handler).await?;
                let host_key =
                    accepted.lock().unwrap_or_else(|e| e.into_inner()).take().ok_or_else(|| {
                        Error::new(ErrorKind::HostKey, format!("{target} did not prove its host key"))
                    })?;
                auth::authenticate(&mut handle, &options.user, credential, &target).await?;
                Ok((handle, host_key))
            }))?;
        Ok(Session { runtime, handle: Some(handle), host_key, target, timeout: options.timeout })
    }

    /// The server's host key, as verified.
    pub fn host_key(&self) -> &HostKey {
        &self.host_key
    }

    /// Runs `argv` on the server and waits for it to end. Each argument is
    /// quoted for the remote shell: it arrives as exactly one argument.
    pub fn run<S: AsRef<str>>(&self, argv: &[S]) -> Result<Output, Error> {
        let command_line = quote::command_line(argv)?;
        let handle = self.handle()?;
        self.runtime.block_on(exec::run(handle, &command_line))
    }

    /// Opens the SFTP subsystem on a channel of this connection.
    pub fn sftp(&self) -> Result<Sftp, Error> {
        let handle = self.handle()?;
        Sftp::open(self.runtime.clone(), handle, &self.target, self.timeout)
    }

    /// Ends the connection politely (dropping the session does too).
    pub fn close(mut self) -> Result<(), Error> {
        self.disconnect()
    }

    fn handle(&self) -> Result<&Handle<Client>, Error> {
        match &self.handle {
            Some(handle) if !handle.is_closed() => Ok(handle),
            _ => Err(Error::new(ErrorKind::Connect, format!("the SSH connection to {} is closed", self.target))),
        }
    }

    fn disconnect(&mut self) -> Result<(), Error> {
        let Some(handle) = self.handle.take() else { return Ok(()) };
        if handle.is_closed() {
            return Ok(());
        }
        self.runtime.block_on(within(Some(Duration::from_secs(2)), || "disconnecting".into(), async {
            handle.disconnect(russh::Disconnect::ByApplication, "", "en").await.map_err(Error::from)
        }))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.disconnect();
    }
}
