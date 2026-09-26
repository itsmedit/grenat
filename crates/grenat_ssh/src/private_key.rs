//! A private key from the text of its file, decrypted with its passphrase
//! when it is encrypted. Errors say what is wrong with the key, never what
//! the key or the passphrase is.

use russh::keys::ssh_key::PrivateKey;

use crate::error::{Error, ErrorKind};

const OPENSSH_HEADER: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";

/// Reads `text`: OpenSSH format (what `ssh-keygen` writes), or else a PEM
/// (PKCS#1, PKCS#8) or PuTTY key.
pub fn decode(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, Error> {
    if text.trim_start().starts_with(OPENSSH_HEADER) {
        return decode_openssh(text, passphrase);
    }
    russh::keys::decode_secret_key(text, passphrase).map_err(|e| match e {
        russh::keys::Error::KeyIsEncrypted => needs_passphrase(),
        _ if passphrase.is_some() => invalid("cannot be read (not a supported key format, or a wrong passphrase)"),
        _ => invalid("cannot be read (OpenSSH, PEM or PuTTY format expected)"),
    })
}

fn decode_openssh(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, Error> {
    let key = PrivateKey::from_openssh(text.trim()).map_err(|_| invalid("is not a valid OpenSSH private key"))?;
    if !key.is_encrypted() {
        return Ok(key);
    }
    match passphrase {
        None => Err(needs_passphrase()),
        Some(passphrase) => key.decrypt(passphrase).map_err(|_| invalid("cannot be decrypted: wrong passphrase")),
    }
}

fn needs_passphrase() -> Error {
    invalid("is encrypted: its passphrase is needed")
}

fn invalid(what: &str) -> Error {
    Error::new(ErrorKind::Auth, format!("the private key {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::LineEnding;
    use russh::keys::ssh_key::private::Ed25519Keypair;

    fn key() -> PrivateKey {
        PrivateKey::from(Ed25519Keypair::from_seed(&[9; 32]))
    }

    #[test]
    fn plain_keys() {
        let text = key().to_openssh(LineEnding::LF).unwrap();
        let decoded = decode(&text, None).unwrap();
        assert_eq!(decoded.public_key().key_data(), key().public_key().key_data());
        // An unneeded passphrase is ignored; surrounding blanks too.
        assert!(decode(&format!("\n  {}\n", text.as_str()), Some("unused")).is_ok());
    }

    #[test]
    fn encrypted_keys() {
        let encrypted = key().encrypt(&mut rand::rng(), "correct horse").unwrap();
        let text = encrypted.to_openssh(LineEnding::LF).unwrap();
        assert!(decode(&text, Some("correct horse")).is_ok());

        let missing = decode(&text, None).unwrap_err();
        assert_eq!(missing.kind(), ErrorKind::Auth);
        assert!(missing.message().contains("passphrase is needed"), "{missing}");

        let wrong = decode(&text, Some("battery staple")).unwrap_err();
        assert_eq!(wrong.kind(), ErrorKind::Auth);
        assert!(wrong.message().contains("wrong passphrase") && !wrong.message().contains("battery"), "{wrong}");
    }

    #[test]
    fn garbage_never_echoed() {
        for text in [
            "not a key at all SECRET",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nSECRET\n-----END OPENSSH PRIVATE KEY-----",
        ] {
            let e = decode(text, None).unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Auth);
            assert!(!e.message().contains("SECRET"), "{e}");
        }
    }
}
