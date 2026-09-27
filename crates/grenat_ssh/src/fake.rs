//! A real SSH server for tests (this crate's and its users'), in process,
//! on 127.0.0.1 and a free port, with a generated host key. User `alice`
//! logs in with the authorized key or the password `s3cret`. A command is
//! recorded exactly as received, then run with `/bin/sh -c "exec …"` in a scratch
//! directory (the server's "home"), which is also the root of its SFTP
//! subsystem. [`socks::Socks`] is a SOCKS5 proxy to reach it through;
//! [`relay::Relay`], a connection to it that a test can cut.

pub mod relay;
pub mod sftp;
pub mod socks;

use std::collections::HashMap;
use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey, PublicKey};
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};

pub const USER: &str = "alice";
pub const PASSWORD: &str = "s3cret";

/// A directory removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> TempDir {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("grenat_ssh_{label}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path.canonicalize().unwrap())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn new_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}

/// The text of `key`'s file, as `ssh-keygen` writes it.
pub fn key_text(key: &PrivateKey) -> String {
    key.to_openssh(LineEnding::LF).unwrap().to_string()
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

pub struct TestServer {
    pub port: u16,
    pub host_key: PrivateKey,
    /// The command strings received, in order.
    pub commands: Arc<Mutex<Vec<String>>>,
    /// The server's home: where commands run and SFTP paths lead.
    pub home: TempDir,
}

impl TestServer {
    /// A server letting `authorized` in, with a fresh host key.
    pub fn start(authorized: &PublicKey) -> TestServer {
        TestServer::start_with_host_key(authorized, new_key())
    }

    pub fn start_with_host_key(authorized: &PublicKey, host_key: PrivateKey) -> TestServer {
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let home = TempDir::new("home");
        let config = Arc::new(russh::server::Config {
            keys: vec![host_key.clone()],
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let mut server =
            SshServer { authorized: authorized.clone(), commands: commands.clone(), home: home.path().to_path_buf() };
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let _ = server.run_on_socket(config, &listener).await;
            });
        });
        TestServer { port, host_key, commands, home }
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(self.host_key.public_key())
    }

    pub fn commands(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }
}

/// A port that accepts TCP connections and never answers: an SSH client
/// waits there forever for the server's greeting.
pub fn silent_port() -> (StdListener, u16) {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// A port nothing listens on.
pub fn closed_port() -> u16 {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[derive(Clone)]
struct SshServer {
    authorized: PublicKey,
    commands: Arc<Mutex<Vec<String>>>,
    home: PathBuf,
}

impl russh::server::Server for SshServer {
    type Handler = Connection;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Connection {
        Connection { server: self.clone(), channels: HashMap::new() }
    }
}

struct Connection {
    server: SshServer,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl russh::server::Handler for Connection {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == USER && password == PASSWORD { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        let known = key.key_data() == self.server.authorized.key_data();
        Ok(if user == USER && known { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).into_owned();
        self.server.commands.lock().unwrap().push(command.clone());
        session.channel_success(channel)?;
        // `exec`: the command replaces the shell, so that a signal ends it as
        // it would end a login shell's last command on every system (dash,
        // Debian's sh, would otherwise report 128 + the signal); what arrives
        // is always one command, its arguments quoted by the client
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("exec {command}"))
            .current_dir(&self.server.home)
            .output()?;
        session.data(channel, output.stdout)?;
        session.extended_data(channel, 1, output.stderr)?;
        match output.status.code() {
            Some(code) => session.exit_status_request(channel, code as u32)?,
            None => session.exit_signal_request(channel, signal(&output.status), false, "", "")?,
        }
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(&mut self, id: ChannelId, name: &str, session: &mut Session) -> Result<(), Self::Error> {
        match self.channels.remove(&id) {
            Some(channel) if name == "sftp" => {
                session.channel_success(id)?;
                russh_sftp::server::run(channel.into_stream(), sftp::Files::new(&self.server.home)).await;
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }
}

fn signal(status: &std::process::ExitStatus) -> russh::Sig {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(9) => russh::Sig::KILL,
        Some(15) => russh::Sig::TERM,
        other => russh::Sig::Custom(format!("{other:?}")),
    }
}
