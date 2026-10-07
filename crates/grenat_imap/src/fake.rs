//! An IMAP server for tests (this crate's and its users'), in process, on
//! 127.0.0.1 and a free port: implicit TLS, STARTTLS, or clear text. User [`USER`] logs in with [`PASSWORD`], or with an
//! OAuth 2.0 token if one is set. Folders hold messages with their UID and
//! flags, which a test reads after the client has run; every command is
//! recorded, with whether it came over TLS. A connection can be made to
//! break ([`FakeImap::drop_at`]) or to go silent ([`FakeImap::stall_at`]).
//!
//! Its certificate is signed by an authority made for the run: a client
//! trusts it only when told to ([`crate::Trust::with`] and
//! [`FakeImap::authority`]), as a test does.

mod session;
mod store;
mod wire;

use std::collections::BTreeSet;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub use store::Stored;

use store::{Fault, State};
use wire::Conn;

pub const USER: &str = "support@acme.com";
pub const PASSWORD: &str = "s3cret";

/// How clients connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// TLS from the first byte (`imaps://`).
    Implicit,
    /// Clear text, STARTTLS offered, LOGIN disabled until then (`imap://`).
    StartTls,
    /// Clear text, nothing offered.
    Plain,
}

/// What the server offers.
#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    /// Announces `MOVE` (and takes `UID MOVE`).
    pub move_command: bool,
    /// Announces `UIDPLUS` (and takes `UID EXPUNGE`).
    pub uidplus: bool,
    /// Announces `LOGINDISABLED` even over TLS.
    pub login_disabled: bool,
    /// Announces `IMAP4rev2` (RFC 9051), which has `MOVE` and `UID EXPUNGE`
    /// without announcing them.
    pub rev2: bool,
    /// The OAuth 2.0 access token `AUTHENTICATE XOAUTH2` takes (announced
    /// as `AUTH=XOAUTH2` only when there is one).
    pub token: Option<String>,
}

impl Config {
    pub fn new(mode: Mode) -> Config {
        Config { mode, move_command: true, uidplus: true, login_disabled: false, rev2: false, token: None }
    }
}

pub struct FakeImap {
    port: u16,
    authority: Vec<u8>,
    state: Arc<Mutex<State>>,
}

impl FakeImap {
    /// A server with an empty `INBOX`.
    pub fn start(config: Config) -> FakeImap {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (authority, tls) = certificates();
        let tls = (config.mode != Mode::Plain).then_some(tls);
        let state = Arc::new(Mutex::new(State::default()));
        state.lock().unwrap().folder("INBOX");
        let shared = state.clone();
        let config = Arc::new(config);
        std::thread::spawn(move || {
            for tcp in listener.incoming().flatten() {
                shared.lock().unwrap().connections += 1;
                let (state, config, tls) = (shared.clone(), config.clone(), tls.clone());
                std::thread::spawn(move || {
                    let conn = match (config.mode, &tls) {
                        (Mode::Implicit, Some(tls)) => match rustls::ServerConnection::new(tls.clone()) {
                            Ok(server) => Conn::Tls(Box::new(rustls::StreamOwned::new(server, tcp))),
                            Err(_) => return,
                        },
                        _ => Conn::Plain(tcp),
                    };
                    session::Session::new(conn, state, config, tls).run();
                });
            }
        });
        FakeImap { port, authority, state }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The certificate of the authority that signed the server's (DER).
    pub fn authority(&self) -> Vec<u8> {
        self.authority.clone()
    }

    /// `scheme://USER:PASSWORD@127.0.0.1:port/folder`, percent-encoded.
    pub fn url(&self, scheme: &str, folder: &str) -> String {
        format!("{scheme}://support%40acme.com:{PASSWORD}@127.0.0.1:{}/{folder}", self.port)
    }

    /// Creates a folder (named as a user names it).
    pub fn create(&self, folder: &str) {
        self.state.lock().unwrap().folder(&crate::encode_folder(folder));
    }

    /// Delivers a message to `folder`, unseen; its UID.
    pub fn deliver(&self, folder: &str, raw: &[u8]) -> u32 {
        self.deliver_with(folder, raw, &[])
    }

    /// Delivers a message with flags (`\Seen`…); its UID.
    pub fn deliver_with(&self, folder: &str, raw: &[u8], flags: &[&str]) -> u32 {
        let flags: BTreeSet<String> = flags.iter().map(|f| f.to_string()).collect();
        self.state.lock().unwrap().folder(&crate::encode_folder(folder)).add(raw.to_vec(), flags)
    }

    /// The messages of `folder`, in order.
    pub fn messages(&self, folder: &str) -> Vec<Stored> {
        self.state.lock().unwrap().folder(&crate::encode_folder(folder)).messages.clone()
    }

    /// The folder's UIDVALIDITY.
    pub fn uid_validity(&self, folder: &str) -> u32 {
        self.state.lock().unwrap().folder(&crate::encode_folder(folder)).uid_validity
    }

    /// Recreates `folder`, empty, with a new UIDVALIDITY.
    pub fn recreate(&self, folder: &str) {
        let mut state = self.state.lock().unwrap();
        let name = crate::encode_folder(folder);
        state.folders.remove(&name);
        state.folder(&name);
    }

    /// The commands received (without their tags), and whether each came over TLS.
    pub fn commands(&self) -> Vec<(bool, String)> {
        self.state.lock().unwrap().commands.clone()
    }

    /// How many connections were accepted.
    pub fn connections(&self) -> usize {
        self.state.lock().unwrap().connections
    }

    /// Closes the connection, unanswered, when the `n`th command from now
    /// arrives (once): a connection that breaks.
    pub fn drop_at(&self, n: usize) {
        self.state.lock().unwrap().fault = Some((n, Fault::Drop));
    }

    /// Stops answering when the `n`th command from now arrives (once),
    /// the connection left open: a server gone silent.
    pub fn stall_at(&self, n: usize) {
        self.state.lock().unwrap().fault = Some((n, Fault::Stall));
    }
}

/// An authority made for the run, and the server's TLS configuration (a
/// certificate it signed for `127.0.0.1` and `localhost`).
fn certificates() -> (Vec<u8>, Arc<rustls::ServerConfig>) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    let authority_key = KeyPair::generate().unwrap();
    let mut authority = CertificateParams::new(Vec::<String>::new()).unwrap();
    authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let authority_cert = authority.self_signed(&authority_key).unwrap();
    let issuer = Issuer::new(authority, authority_key);
    let server_key = KeyPair::generate().unwrap();
    let server = CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()]).unwrap();
    let server_cert = server.signed_by(&server_key, &issuer).unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(server_cert.der().to_vec())], key)
        .unwrap();
    (authority_cert.der().to_vec(), Arc::new(config))
}
