//! Encrypted credentials, as in Rails: YAML encrypted with AES-256-GCM.
//!
//! - `config/credentials.yml.enc`, its key `config/master.key`;
//! - for one environment: `config/credentials/<env>.yml.enc`, its key
//!   `config/credentials/<env>.key` — used instead when it exists;
//! - `GRENAT_MASTER_KEY` gives the key without a file (production).
//!
//! A file is `<ciphertext>--<nonce>`, both in base64; the key is 32 random
//! bytes in hex. The text is kept as written, comments included.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value as Json;

/// The key, when it is not in a file.
pub const KEY_VARIABLE: &str = "GRENAT_MASTER_KEY";

/// What a new credentials file holds.
pub const TEMPLATE: &str = "\
# The application's secrets, encrypted in this file: `grenat credentials edit`.
# In code: Credentials.fetch(:github, :token) — a secret, never shown, never
# sent to a model. Model providers find their keys here (anthropic.api_key…).
#
# anthropic:
#   api_key: sk-ant-…
# openai:
#   api_key: sk-…
# github:
#   token: ghp_…
";

/// Where credentials are: the encrypted file and its key.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub file: PathBuf,
    pub key: PathBuf,
}

impl Location {
    /// The credentials of `env` (its own files), or the application's when
    /// `env` is `None`.
    pub fn of(root: &Path, env: Option<&str>) -> Location {
        let config = root.join("config");
        match env {
            Some(env) => {
                let dir = config.join("credentials");
                Location { file: dir.join(format!("{env}.yml.enc")), key: dir.join(format!("{env}.key")) }
            }
            None => Location { file: config.join("credentials.yml.enc"), key: config.join("master.key") },
        }
    }

    /// The credentials used in `env`: its own if they exist, else the
    /// application's.
    pub fn for_env(root: &Path, env: &str) -> Location {
        let own = Location::of(root, Some(env));
        if own.file.exists() { own } else { Location::of(root, None) }
    }

    pub fn exists(&self) -> bool {
        self.file.exists()
    }

    /// The key: `GRENAT_MASTER_KEY`, else the key file.
    pub fn key(&self) -> Result<[u8; 32], String> {
        let text = match std::env::var(KEY_VARIABLE).ok().filter(|k| !k.trim().is_empty()) {
            Some(key) => key,
            None => std::fs::read_to_string(&self.key).map_err(|_| {
                format!("no key for {}: set {KEY_VARIABLE}, or put it in {}", self.file.display(), self.key.display())
            })?,
        };
        parse_key(text.trim())
    }

    /// The decrypted text.
    pub fn read(&self) -> Result<String, String> {
        let data = std::fs::read_to_string(&self.file).map_err(|e| format!("cannot read {}: {e}", self.file.display()))?;
        decrypt(&self.key()?, data.trim()).map_err(|e| format!("{}: {e}", self.file.display()))
    }

    /// The credentials, as a tree.
    pub fn load(&self) -> Result<Json, String> {
        crate::yaml::parse(&self.read()?).map_err(|e| format!("{}: {e}", self.file.display()))
    }

    /// Encrypts and writes `text`, if it is valid YAML.
    pub fn write(&self, text: &str) -> Result<(), String> {
        crate::yaml::parse(text)?;
        let key = self.key()?;
        std::fs::write(&self.file, encrypt(&key, text) + "\n").map_err(|e| format!("cannot write {}: {e}", self.file.display()))
    }

    /// A new key file and credentials holding [`TEMPLATE`]; fails if either
    /// exists.
    pub fn create(&self) -> Result<(), String> {
        for path in [&self.file, &self.key] {
            if path.exists() {
                return Err(format!("{} already exists", path.display()));
            }
        }
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        write_private(&self.key, &(new_key() + "\n"))?;
        let key = self.key()?;
        std::fs::write(&self.file, encrypt(&key, TEMPLATE) + "\n").map_err(|e| format!("cannot write {}: {e}", self.file.display()))
    }
}

/// What `.gitignore` must hold: keys never go to the repository.
pub const IGNORED: [&str; 2] = ["config/master.key", "config/credentials/*.key"];

/// Adds [`IGNORED`] to `root/.gitignore` (created if needed); whether it
/// changed.
pub fn ignore_keys(root: &Path) -> Result<bool, String> {
    let path = root.join(".gitignore");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = IGNORED.iter().copied().filter(|p| !text.lines().any(|l| l.trim() == *p)).collect();
    if missing.is_empty() {
        return Ok(false);
    }
    let separator = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
    let text = format!("{text}{separator}{}\n", missing.join("\n"));
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
}

/// A new key: 32 random bytes, in hex.
pub fn new_key() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("the system's random numbers");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_key(text: &str) -> Result<[u8; 32], String> {
    let invalid = || "invalid key: 64 hexadecimal digits expected".to_string();
    if text.len() != 64 {
        return Err(invalid());
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(2 * i..2 * i + 2).ok_or_else(invalid)?, 16).map_err(|_| invalid())?;
    }
    Ok(key)
}

pub fn encrypt(key: &[u8; 32], text: &str) -> String {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce).expect("the system's random numbers");
    let sealed = cipher.encrypt(Nonce::from_slice(&nonce), text.as_bytes()).expect("encryption of a text");
    format!("{}--{}", BASE64.encode(sealed), BASE64.encode(nonce))
}

pub fn decrypt(key: &[u8; 32], data: &str) -> Result<String, String> {
    let (sealed, nonce) = data.split_once("--").ok_or("not an encrypted credentials file")?;
    let sealed = BASE64.decode(sealed).map_err(|_| "not an encrypted credentials file")?;
    let nonce = BASE64.decode(nonce).ok().filter(|n| n.len() == 12).ok_or("not an encrypted credentials file")?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let text = cipher
        .decrypt(Nonce::from_slice(&nonce), sealed.as_slice())
        .map_err(|_| "cannot decrypt: wrong key, or the file was changed")?;
    String::from_utf8(text).map_err(|_| "cannot decrypt: not text".to_string())
}

/// A file only its owner reads.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(fail)?;
        file.write_all(text.as_bytes()).map_err(fail)
    }
    #[cfg(not(unix))]
    std::fs::write(path, text).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_round_trip_and_tampering() {
        let key = parse_key(&new_key()).unwrap();
        let sealed = encrypt(&key, "a: 1\n");
        assert_eq!(decrypt(&key, &sealed).unwrap(), "a: 1\n");
        assert_ne!(encrypt(&key, "a: 1\n"), sealed, "a new nonce each time");
        let other = parse_key(&new_key()).unwrap();
        assert!(decrypt(&other, &sealed).unwrap_err().contains("wrong key"));
        let (body, nonce) = sealed.split_once("--").unwrap();
        let mut bytes = BASE64.decode(body).unwrap();
        bytes[0] ^= 1;
        assert!(decrypt(&key, &format!("{}--{nonce}", BASE64.encode(bytes))).unwrap_err().contains("changed"));
        assert!(decrypt(&key, "plain text").is_err());
        assert!(parse_key("abc").is_err());
        assert!(parse_key(&"zz".repeat(32)).is_err());
    }
}
