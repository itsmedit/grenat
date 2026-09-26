//! The errors of SSH and SFTP, as programs rescue them: `SshError` (the
//! connection, a command, the proxy), `HostKeyError`, `SshAuthError`,
//! `SftpError` and `TimeoutError`. An unknown host key says how to trust it;
//! a secret proxy is named `[secret]`.

use grenat_ssh::{Error, ErrorKind};

use crate::prelude::*;

/// The language error of `e`.
pub(super) fn ssh_error<'p, T>(e: Error) -> Result<T, Ctrl<'p>> {
    let message = match e.kind() {
        ErrorKind::HostKey => with_trust_hint(e.message()),
        _ => e.message().to_string(),
    };
    raise(error_type(e.kind()), message)
}

pub(super) fn error_type(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Connect | ErrorKind::Command | ErrorKind::Proxy => "SshError",
        ErrorKind::HostKey => "HostKeyError",
        ErrorKind::Auth => "SshAuthError",
        ErrorKind::Sftp => "SftpError",
        ErrorKind::Timeout => "TimeoutError",
    }
}

/// What `grenat_ssh` suggests for an unknown key, said in the language's
/// terms: the options of `Ssh.connect` that trust it. A changed or revoked
/// key gets no such advice.
fn with_trust_hint(message: &str) -> String {
    const GENERIC: &str = "then expect it (fingerprint) or record it (record into a known_hosts file)";
    if !message.contains(" is unknown") {
        return message.to_string();
    }
    let fingerprint = message
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|word| word.starts_with("SHA256:"))
        .unwrap_or("SHA256:…");
    let hint =
        format!("pass `fingerprint: \"{fingerprint}\"` to `Ssh.connect`, or `known_hosts:` a file that records it");
    match message.find(GENERIC) {
        Some(at) => format!("{}then {hint}", &message[..at]),
        None => format!("{message}. To trust it once checked, {hint}"),
    }
}

/// `message` without the secret proxy URL `url`: neither the URL, nor its
/// password, nor its address.
pub(super) fn hide_proxy(message: &str, url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest).trim_end_matches('/');
    let (credentials, address) = rest.rsplit_once('@').map_or((None, rest), |(c, a)| (Some(c), a));
    let mut hidden = message.replace(url, "[secret]");
    if let Some(password) = credentials.and_then(|c| c.split_once(':')).map(|(_, p)| p).filter(|p| !p.is_empty()) {
        hidden = hidden.replace(password, "[secret]");
    }
    if !address.is_empty() {
        let with_port = if address.ends_with(']') || !address.contains(':') {
            format!("{address}:1080")
        } else {
            address.to_string()
        };
        hidden = hidden.replace(&with_port, "[secret]").replace(address, "[secret]");
    }
    hidden
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message<'p>(result: Result<(), Ctrl<'p>>) -> (String, String) {
        match result {
            Err(Ctrl::Raise(e)) => (e.ty.to_string(), e.message.clone()),
            _ => panic!("expected an error"),
        }
    }

    #[test]
    fn kinds_become_error_types() {
        let (ty, text) = message(ssh_error(Error::new(ErrorKind::Auth, "api:22 refused the key")));
        assert_eq!((ty.as_str(), text.as_str()), ("SshAuthError", "api:22 refused the key"));
        assert_eq!(error_type(ErrorKind::Sftp), "SftpError");
        assert_eq!(error_type(ErrorKind::Timeout), "TimeoutError");
        assert_eq!(error_type(ErrorKind::Proxy), "SshError");
    }

    #[test]
    fn an_unknown_key_says_how_to_trust_it() {
        let unknown = "the host key of api is unknown: it offers ssh-ed25519 SHA256:abc/+x, which /h/known_hosts does \
                       not record. Check this fingerprint with the server's administrator, then expect it \
                       (fingerprint) or record it (record into a known_hosts file)";
        let (ty, text) = message(ssh_error(Error::new(ErrorKind::HostKey, unknown)));
        assert_eq!(ty, "HostKeyError");
        assert!(
            text.ends_with(
                "administrator, then pass `fingerprint: \"SHA256:abc/+x\"` to `Ssh.connect`, or `known_hosts:` a \
                 file that records it"
            ),
            "{text}"
        );
        let fallback = with_trust_hint("x is unknown: SHA256:k");
        assert!(fallback.ends_with(". To trust it once checked, pass `fingerprint: \"SHA256:k\"` to `Ssh.connect`, or `known_hosts:` a file that records it"), "{fallback}");
        let changed = "the host key of api has CHANGED: it now offers ssh-ed25519 SHA256:abc";
        assert_eq!(with_trust_hint(changed), changed);
    }

    #[test]
    fn a_secret_proxy_is_never_named() {
        let url = "socks5://bob:pr0xy@10.0.0.9:1081";
        assert_eq!(
            hide_proxy("cannot reach the proxy 10.0.0.9:1081: refused", url),
            "cannot reach the proxy [secret]: refused"
        );
        assert_eq!(
            hide_proxy("the proxy proxy.corp:1080 said pr0xy", "socks5://u:pr0xy@proxy.corp"),
            "the proxy [secret] said [secret]"
        );
        assert_eq!(hide_proxy("bad socks5://h:1", "socks5://h:1"), "bad [secret]");
    }
}
