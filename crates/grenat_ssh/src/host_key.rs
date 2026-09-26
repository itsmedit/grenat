//! Whether to trust the host key a server offered, under a [`KnownHosts`]
//! policy. Every refusal names the key offered (algorithm and SHA256
//! fingerprint) so that a user can check it out of band.

use std::io;
use std::path::Path;

use russh::keys::ssh_key::{HashAlg, PublicKey};

use crate::error::{Error, ErrorKind};
use crate::host_pattern::host_name;
use crate::known_hosts::{self, Lookup};
use crate::options::KnownHosts;

/// The key a session's server proved it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    /// `ssh-ed25519`, `ecdsa-sha2-nistp256`, `rsa-sha2-512`…
    pub algorithm: String,
    /// `SHA256:…`, as `ssh-keygen -l` prints it.
    pub fingerprint: String,
    /// Whether this connection recorded it ([`KnownHosts::RecordInto`], first use).
    pub recorded: bool,
}

/// Accepts `key`, offered by `host:port`, or says why not.
pub fn verify(policy: &KnownHosts, host: &str, port: u16, key: &PublicKey) -> Result<HostKey, Error> {
    let offered = HostKey {
        algorithm: key.algorithm().as_str().to_string(),
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        recorded: false,
    };
    let server = if port == 22 { host.to_string() } else { format!("{host}:{port}") };
    let described = format!("{} {}", offered.algorithm, offered.fingerprint);
    match policy {
        KnownHosts::Fingerprint(expected) => {
            if same_fingerprint(expected, &offered.fingerprint) {
                Ok(offered)
            } else {
                Err(refused(format!(
                    "the host key of {server} is {described}, not the expected {}: the connection is refused",
                    expected.trim()
                )))
            }
        }
        KnownHosts::File(path) | KnownHosts::RecordInto(path) => {
            if path.as_os_str().is_empty() {
                return Err(refused(format!(
                    "no known_hosts file (the home directory is unknown) to check the host key of {server} ({described})"
                )));
            }
            let name = host_name(host, port);
            let text = read(path, &server)?;
            match known_hosts::lookup(&text, &name, key) {
                Lookup::Known => Ok(offered),
                Lookup::Changed { line } => Err(refused(format!(
                    "the host key of {server} has CHANGED: it now offers {described}, but {} line {line} records \
                     another key. Someone may be intercepting the connection, or the server was reinstalled; the \
                     connection is refused",
                    path.display()
                ))),
                Lookup::Revoked { line } => Err(refused(format!(
                    "the host key of {server} ({described}) is revoked ({} line {line}): the connection is refused",
                    path.display()
                ))),
                Lookup::Unknown if matches!(policy, KnownHosts::RecordInto(_)) => {
                    known_hosts::append(path, &known_hosts::line(&name, key)).map_err(|e| {
                        refused(format!("cannot record the host key of {server} into {}: {e}", path.display()))
                    })?;
                    Ok(HostKey { recorded: true, ..offered })
                }
                Lookup::Unknown => Err(refused(format!(
                    "the host key of {server} is unknown: it offers {described}, which {} does not record. Check \
                     this fingerprint with the server's administrator, then expect it (fingerprint) or record it \
                     (record into a known_hosts file)",
                    path.display()
                ))),
            }
        }
    }
}

fn refused(message: String) -> Error {
    Error::new(ErrorKind::HostKey, message)
}

/// The file's text; a missing file records nothing.
fn read(path: &Path, server: &str) -> Result<String, Error> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(refused(format!("cannot read {} to check the host key of {server}: {e}", path.display()))),
    }
}

/// `SHA256:abc` and `abc` name the same fingerprint; base64 padding is optional.
fn same_fingerprint(expected: &str, offered: &str) -> bool {
    let normalize = |f: &str| {
        let f = f.trim();
        let f = f.strip_prefix("SHA256:").unwrap_or(f);
        f.trim_end_matches('=').to_string()
    };
    let expected = normalize(expected);
    !expected.is_empty() && expected == normalize(offered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::private::{Ed25519Keypair, PrivateKey};
    use std::path::PathBuf;

    fn key(seed: u8) -> PublicKey {
        PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32])).public_key().clone()
    }

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str) -> TempFile {
            let path = std::env::temp_dir().join(format!("grenat_ssh_host_key_{}_{name}", std::process::id()));
            let _ = std::fs::remove_file(&path);
            TempFile(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn fingerprints() {
        let k = key(1);
        let fingerprint = k.fingerprint(HashAlg::Sha256).to_string();
        assert!(fingerprint.starts_with("SHA256:"));
        let ok = verify(&KnownHosts::Fingerprint(fingerprint.clone()), "h", 22, &k).unwrap();
        assert_eq!(
            (ok.algorithm.as_str(), ok.fingerprint.as_str(), ok.recorded),
            ("ssh-ed25519", &*fingerprint, false)
        );
        let bare = fingerprint.trim_start_matches("SHA256:").to_string();
        assert!(verify(&KnownHosts::Fingerprint(format!(" {bare}= ")), "h", 22, &k).is_ok());

        let other = key(2).fingerprint(HashAlg::Sha256).to_string();
        let e = verify(&KnownHosts::Fingerprint(other.clone()), "h", 2222, &k).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::HostKey);
        assert!(e.message().contains(&fingerprint) && e.message().contains(&other), "{e}");
        assert!(e.message().contains("h:2222"), "{e}");
        assert!(verify(&KnownHosts::Fingerprint(String::new()), "h", 22, &k).is_err());
        assert!(verify(&KnownHosts::Fingerprint("SHA256:".into()), "h", 22, &k).is_err());
    }

    #[test]
    fn strict_file() {
        let file = TempFile::new("strict");
        let policy = KnownHosts::File(file.0.clone());
        let k = key(1);
        let fingerprint = k.fingerprint(HashAlg::Sha256).to_string();

        let unknown = verify(&policy, "example.com", 22, &k).unwrap_err();
        assert_eq!(unknown.kind(), ErrorKind::HostKey);
        assert!(unknown.message().contains("unknown") && unknown.message().contains(&fingerprint), "{unknown}");
        assert!(!file.0.exists(), "a strict policy never writes");

        std::fs::write(&file.0, known_hosts::line("example.com", &k)).unwrap();
        assert!(!verify(&policy, "example.com", 22, &k).unwrap().recorded);

        let changed = verify(&policy, "example.com", 22, &key(2)).unwrap_err();
        assert_eq!(changed.kind(), ErrorKind::HostKey);
        assert!(changed.message().contains("CHANGED") && changed.message().contains("line 1"), "{changed}");
        assert!(changed.message().contains(&key(2).fingerprint(HashAlg::Sha256).to_string()), "{changed}");

        assert!(verify(&KnownHosts::File(PathBuf::new()), "example.com", 22, &k).is_err());
    }

    #[test]
    fn trust_on_first_use() {
        let file = TempFile::new("tofu");
        let policy = KnownHosts::RecordInto(file.0.clone());
        let k = key(1);
        assert!(verify(&policy, "10.0.0.1", 2222, &k).unwrap().recorded);
        let text = std::fs::read_to_string(&file.0).unwrap();
        assert!(text.starts_with("[10.0.0.1]:2222 ssh-ed25519 "), "{text}");
        // Known now: accepted, not recorded twice.
        assert!(!verify(&policy, "10.0.0.1", 2222, &k).unwrap().recorded);
        assert!(!verify(&KnownHosts::File(file.0.clone()), "10.0.0.1", 2222, &k).unwrap().recorded);
        assert_eq!(std::fs::read_to_string(&file.0).unwrap(), text);
        // A changed key is refused even here, and the file is left alone.
        let e = verify(&policy, "10.0.0.1", 2222, &key(2)).unwrap_err();
        assert!(e.message().contains("CHANGED"), "{e}");
        assert_eq!(std::fs::read_to_string(&file.0).unwrap(), text);
    }

    #[test]
    fn revoked() {
        let file = TempFile::new("revoked");
        let k = key(1);
        let openssh = PublicKey::new(k.key_data().clone(), "").to_openssh().unwrap();
        std::fs::write(&file.0, format!("{}\n@revoked * {openssh}\n", known_hosts::line("example.com", &k))).unwrap();
        let e = verify(&KnownHosts::RecordInto(file.0.clone()), "example.com", 22, &k).unwrap_err();
        assert!(e.message().contains("revoked") && e.message().contains("line 2"), "{e}");
    }
}
