//! `Ssh.connect("user@host", key: …)`: the options of a connection, the
//! connection itself (a real one, or the test's `mock_ssh` double) and the
//! `SshSession` record that stands for it.

use std::time::Duration;

use grenat_ssh::{Auth, KnownHosts, Options, Session};

use super::errors::{self, hide_proxy};
use crate::prelude::*;
use crate::ssh::{Connection, Live, SshEntry, Target};

/// The record `Ssh.connect` returns.
pub(crate) const SSH_SESSION: &str = "SshSession";

const DEFAULT_PORT: u16 = 22;

pub(crate) fn call_ssh<'p>(interp: &mut Interp<'p>, name: &str, args: Args<'p>) -> R<'p> {
    if name != "connect" {
        return raise("NoMethodError", format!("unknown method `Ssh.{name}`"));
    }
    if args.pos.iter().chain(args.named.iter().map(|(_, v)| v)).any(Value::contains_taint) {
        return raise("TaintError", "an untrusted value reaches `Ssh.connect` (effect `ssh`) without validation");
    }
    let target = match args.pos.first() {
        Some(Value::Str(text)) => Target::parse(text),
        _ => None,
    };
    let Some(mut target) = target else {
        let given = args.pos.first().map_or_else(|| "nothing".into(), Value::inspect);
        return raise("ArgumentError", format!("`Ssh.connect` expects \"user@host\", got {given}"));
    };
    interp.check_ssh(&target.host)?;
    let (options, proxy) = options(&target, &args)?;
    target.port = Some(options.port);
    let label = target.label();
    let stub = interp.ssh_stubs.borrow().get(&label).cloned();
    let (connection, how) = match stub {
        Some(double) => (Connection::Double(double), "(mock)".to_string()),
        None if interp.offline => {
            return raise("SshError", format!("no SSH in tests: `{label}` is not stubbed with `mock_ssh`"));
        }
        None => {
            let session = match grenat_green::blocking(|| Session::connect(options)) {
                Ok(session) => session,
                Err(e) => {
                    let message = match &proxy {
                        Some(Proxy { url, secret: true }) => hide_proxy(e.message(), url),
                        _ => e.message().to_string(),
                    };
                    return errors::ssh_error(grenat_ssh::Error::new(e.kind(), message));
                }
            };
            let key = session.host_key();
            let how = format!("({} {})", key.algorithm, key.fingerprint);
            (Connection::Real(Box::new(Live { session, sftp: None })), how)
        }
    };
    if interp.log {
        let via = match &proxy {
            Some(Proxy { secret: true, .. }) => " via [secret]".to_string(),
            Some(Proxy { url, .. }) => format!(" via {}", without_credentials(url)),
            None => String::new(),
        };
        let port = target.port.unwrap_or(DEFAULT_PORT);
        interp.write_err(&format!("[ssh] {label}:{port}: connected {how}{via}\n"));
    }
    Ok(interp.register_ssh(target, connection))
}

impl<'p> Interp<'p> {
    /// A connection, as the `SshSession` record programs use.
    fn register_ssh(&self, target: Target, connection: Connection) -> Value<'p> {
        let mut sessions = self.ssh_sessions.borrow_mut();
        let fields = vec![
            ("id".into(), Value::Int(sessions.len() as i64)),
            ("user".into(), Value::str(&target.user)),
            ("host".into(), Value::str(&target.host)),
            ("port".into(), Value::Int(i64::from(target.port.unwrap_or(DEFAULT_PORT)))),
        ];
        sessions.push(SshEntry::new(target, connection));
        Value::record(SSH_SESSION, fields)
    }
}

/// The proxy of a connection, and whether its URL is a secret.
struct Proxy {
    url: String,
    secret: bool,
}

/// What `Ssh.connect`'s named arguments ask for: `key:` (and `passphrase:`)
/// or `password:`, `port:`, `proxy:`, `known_hosts:` or `fingerprint:`, `timeout:`.
fn options<'p>(target: &Target, args: &Args<'p>) -> Result<(Options, Option<Proxy>), Ctrl<'p>> {
    let invalid = |option: &str, value: &Value<'p>| {
        raise("ArgumentError", format!("invalid `Ssh.connect` option `{option}: {}`", value.inspect()))
    };
    let (mut key, mut passphrase, mut password, mut port) = (None, None, None, target.port);
    let mut options = Options::new(&target.user, &target.host, Auth::Password(String::new()));
    let mut proxy = None;
    let mut trust = None;
    for (option, value) in &args.named {
        match (option.as_str(), value) {
            ("key", Value::Str(text) | Value::Secret(text)) => key = Some(text.to_string()),
            ("passphrase", Value::Str(text) | Value::Secret(text)) => passphrase = Some(text.to_string()),
            ("password", Value::Str(text) | Value::Secret(text)) => password = Some(text.to_string()),
            ("port", Value::Int(n)) if (1..=65535).contains(n) => {
                if port.is_some_and(|p| i64::from(p) != *n) {
                    return raise(
                        "ArgumentError",
                        "`Ssh.connect`: the port is given twice (in the target and `port:`)",
                    );
                }
                port = Some(*n as u16);
            }
            ("proxy", Value::Nil | Value::Bool(false)) => proxy = None,
            ("proxy", Value::Str(url) | Value::Secret(url)) => {
                let secret = matches!(value, Value::Secret(_));
                if !url.starts_with("socks5://") && !url.starts_with("socks5h://") {
                    let shown = if secret { "[secret]".to_string() } else { format!("{:?}", without_credentials(url)) };
                    return raise(
                        "ArgumentError",
                        format!("`Ssh.connect` goes through SOCKS5 proxies only (socks5://…), got {shown}"),
                    );
                }
                proxy = Some(Proxy { url: url.to_string(), secret });
            }
            ("known_hosts" | "fingerprint", _) if trust.is_some() => {
                return raise("ArgumentError", "`Ssh.connect` takes `known_hosts:` or `fingerprint:`, not both");
            }
            ("known_hosts", Value::Str(path)) => trust = Some(KnownHosts::File(expand_home(path))),
            ("fingerprint", Value::Str(fingerprint)) => trust = Some(KnownHosts::Fingerprint(fingerprint.to_string())),
            ("timeout", Value::Int(n)) if *n > 0 => options.timeout = Some(Duration::from_secs(*n as u64)),
            ("timeout", Value::Float(s) | Value::Duration(s)) if *s > 0.0 => match Duration::try_from_secs_f64(*s) {
                Ok(limit) => options.timeout = Some(limit),
                Err(_) => return invalid(option, value),
            },
            (option, value) => return invalid(option, value),
        }
    }
    options.auth = match (key, password) {
        (Some(text), None) => Auth::Key { text, passphrase },
        (None, Some(password)) if passphrase.is_none() => Auth::Password(password),
        (None, Some(_)) => return raise("ArgumentError", "`Ssh.connect`: a `passphrase:` goes with a `key:`"),
        (Some(_), Some(_)) => return raise("ArgumentError", "`Ssh.connect` takes `key:` or `password:`, not both"),
        (None, None) => {
            return raise(
                "ArgumentError",
                "`Ssh.connect` needs credentials: `key: Credentials.fetch(:deploy, :ssh_key)` or `password: …`",
            );
        }
    };
    options.port = port.unwrap_or(DEFAULT_PORT);
    options.known_hosts = trust.unwrap_or_default();
    options.proxy = proxy.as_ref().map(|p| p.url.clone());
    Ok((options, proxy))
}

/// A proxy URL without its user name and password, with or without a
/// scheme (`http://bob:pw@proxy:8080` → `http://proxy:8080`).
fn without_credentials(url: &str) -> String {
    if url.contains("://") {
        crate::http::without_credentials(url)
    } else {
        url.rsplit('@').next().unwrap_or_default().to_string()
    }
}

/// `~/x` → `<home>/x`.
fn expand_home(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => path.into(),
    }
}
