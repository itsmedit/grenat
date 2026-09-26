//! SSH and SFTP: run commands on a server and handle its files. Nothing
//! here knows about the Grenat language.
//!
//! **A blocking API.** [`Session::connect`] returns once the connection is
//! authenticated; [`Session::run`] once the command has ended; each
//! [`Sftp`] call once the server has answered. Underneath, the SSH library
//! is asynchronous: every session owns a private current-thread tokio
//! runtime, driven only while one of its calls blocks. Callers (the
//! interpreter's green threads, inside a blocking section) never see it —
//! and must not call from inside a tokio runtime of their own. Pure Rust
//! throughout (`russh`, `russh-sftp`, `ring`): no system library.
//!
//! **Host keys are verified before any credential is sent.** [`KnownHosts`]
//! says how: the key must be in a known_hosts file (`~/.ssh/known_hosts` by
//! default); or have a given `SHA256:…` fingerprint; or, trust on first use,
//! a host not yet in a file has its key recorded there — explicitly asked
//! for, and reported by [`HostKey::recorded`]. An unknown, changed or
//! revoked key is an [`ErrorKind::HostKey`] error naming the key offered
//! (algorithm and fingerprint), so that it can be checked out of band.
//!
//! **Credentials are secrets.** A private key is given as text (OpenSSH
//! format, optionally encrypted with a passphrase) or a password is; none
//! of them, nor the proxy's credentials, ever appears in an error message
//! or in `Debug` output.
//!
//! **Commands are argv, quoted by this crate.** SSH carries one command
//! string, which the server's POSIX shell splits again; [`Session::run`]
//! takes the arguments separately and single-quotes each one, so that an
//! argument (`; rm -rf /`, `$(id)`, a quote, a newline…) always arrives as
//! that one argument and never as shell syntax.
//!
//! **Network.** A connection may go through a SOCKS5 proxy
//! (`socks5://[user:password@]host:port`) and has a timeout covering TCP,
//! the proxy, the handshake and authentication (30 s by default).
//!
//! **Tests.** The `fake` feature adds the `fake` module: a real SSH and
//! SFTP server in process, and a SOCKS5 proxy, for the tests of this crate
//! and of its users.

mod auth;
mod error;
mod exec;
#[cfg(feature = "fake")]
pub mod fake;
mod handler;
mod host_key;
mod host_pattern;
mod known_hosts;
mod options;
mod private_key;
pub mod quote;
mod runtime;
mod session;
mod sftp;
mod socks5;
mod transport;

pub use error::{Error, ErrorKind};
pub use exec::Output;
pub use host_key::HostKey;
pub use options::{Auth, KnownHosts, Options};
pub use session::Session;
pub use sftp::{Entry, Sftp};
