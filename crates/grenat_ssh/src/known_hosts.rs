//! The known_hosts file: what it says about a host's key, and how a new
//! key is recorded into it. Lines this crate cannot read (another key
//! type, a malformed line) are skipped, as OpenSSH does; host certificates
//! (`@cert-authority`) are not supported and never make a key known.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use russh::keys::ssh_key::PublicKey;
use russh::keys::ssh_key::known_hosts::{Entry, Marker};

use crate::host_pattern;

/// What the file says about the key a host offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// A line for this host has this very key.
    Known,
    /// No line for this host.
    Unknown,
    /// Lines for this host, none with this key: the first such line.
    Changed { line: usize },
    /// The key is marked `@revoked` (for any host), at this line.
    Revoked { line: usize },
}

/// What `text` (a known_hosts file) says about `key` offered by `name` (a
/// [`host_pattern::host_name`]). Lines are numbered from 1.
pub fn lookup(text: &str, name: &str, key: &PublicKey) -> Lookup {
    let mut known = false;
    let mut changed = None;
    for (index, line) in text.lines().enumerate() {
        let Some(entry) = parse(line) else { continue };
        let same_key = entry.public_key().key_data() == key.key_data();
        match entry.marker() {
            Some(Marker::Revoked) if same_key => return Lookup::Revoked { line: index + 1 },
            Some(_) => continue,
            None => {}
        }
        if host_pattern::matches(entry.host_patterns(), name) {
            if same_key {
                known = true;
            } else {
                changed.get_or_insert(index + 1);
            }
        }
    }
    match (known, changed) {
        (true, _) => Lookup::Known,
        (false, Some(line)) => Lookup::Changed { line },
        (false, None) => Lookup::Unknown,
    }
}

/// A line of the file, fields separated by single spaces (the parser wants
/// them so), or `None` for a blank line, a comment or a line it cannot read.
fn parse(line: &str) -> Option<Entry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    line.split_whitespace().collect::<Vec<_>>().join(" ").parse().ok()
}

/// The line recording `key` for `name`: `name algorithm base64`.
pub fn line(name: &str, key: &PublicKey) -> String {
    let bare = PublicKey::new(key.key_data().clone(), "");
    let openssh = bare.to_openssh().unwrap_or_default();
    format!("{name} {}", openssh.trim_end())
}

/// Appends `line` to the file at `path`, creating it (and its directory) if
/// needed, on a line of its own.
pub fn append(path: &Path, line: &str) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().read(true).append(true).create(true).open(path)?;
    let mut last = [0u8; 1];
    let needs_newline = file.seek(SeekFrom::End(-1)).is_ok() && file.read_exact(&mut last).is_ok() && last[0] != b'\n';
    let mut text = String::new();
    if needs_newline {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    file.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::private::{Ed25519Keypair, PrivateKey};

    fn key(seed: u8) -> PublicKey {
        PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32])).public_key().clone()
    }

    fn file(lines: &[String]) -> String {
        lines.join("\n")
    }

    #[test]
    fn known_unknown_changed() {
        let (a, b) = (key(1), key(2));
        let text = file(&[
            "# a comment".into(),
            String::new(),
            line("other.org", &b),
            line("example.com", &a),
            line("[127.0.0.1]:2222", &b),
        ]);
        assert_eq!(lookup(&text, "example.com", &a), Lookup::Known);
        assert_eq!(lookup(&text, "example.com", &b), Lookup::Changed { line: 4 });
        assert_eq!(lookup(&text, "nowhere.net", &a), Lookup::Unknown);
        assert_eq!(lookup(&text, "[127.0.0.1]:2222", &b), Lookup::Known);
        assert_eq!(lookup(&text, "127.0.0.1", &b), Lookup::Unknown);
        assert_eq!(lookup("", "example.com", &a), Lookup::Unknown);
    }

    #[test]
    fn a_host_with_several_keys_is_known_by_any() {
        let (a, b) = (key(1), key(2));
        let text = file(&[line("example.com", &a), line("example.com", &b)]);
        assert_eq!(lookup(&text, "example.com", &b), Lookup::Known);
        assert_eq!(lookup(&text, "example.com", &key(3)), Lookup::Changed { line: 1 });
    }

    #[test]
    fn revoked_keys_and_markers() {
        let (a, b) = (key(1), key(2));
        let openssh = PublicKey::new(a.key_data().clone(), "").to_openssh().unwrap();
        let text = file(&[
            line("example.com", &a),
            format!("@revoked * {openssh}"),
            format!("@cert-authority *.example.com {}", PublicKey::new(b.key_data().clone(), "").to_openssh().unwrap()),
        ]);
        assert_eq!(lookup(&text, "example.com", &a), Lookup::Revoked { line: 2 });
        // A CA line is not a host key: `b` stays unknown for db.example.com.
        assert_eq!(lookup(&text, "db.example.com", &b), Lookup::Unknown);
    }

    #[test]
    fn tolerant_parsing() {
        let a = key(1);
        let openssh = PublicKey::new(a.key_data().clone(), "").to_openssh().unwrap();
        let spaced = openssh.replace(' ', "\t  ");
        let text = file(&[
            "garbage line".into(),
            "example.com ssh-unknown AAAA".into(),
            format!("  example.com\t{spaced} a comment  "),
        ]);
        assert_eq!(lookup(&text, "example.com", &a), Lookup::Known);
    }

    #[test]
    fn recorded_lines() {
        let a = key(1);
        let l = line("[10.0.0.1]:2222", &a);
        assert!(l.starts_with("[10.0.0.1]:2222 ssh-ed25519 AAAA"), "{l}");
        assert_eq!(l.split(' ').count(), 3);
    }

    #[test]
    fn appending() {
        let dir = std::env::temp_dir().join(format!("grenat_ssh_known_hosts_{}", std::process::id()));
        let path = dir.join("sub").join("known_hosts");
        append(&path, "one").unwrap();
        append(&path, "two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        fs::write(&path, "no newline").unwrap();
        append(&path, "three").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "no newline\nthree\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
