//! A file's content, told apart from another: its SHA-256, in hex, read in
//! pieces (a large file is never held in memory) from the file opened — by
//! [`open_inside`](crate::open_inside), never by its name again.

use std::io::{self, Read};

use sha2::{Digest, Sha256};

/// The SHA-256 of what `file` holds from where it stands, in lowercase hex.
pub fn digest(mut file: impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sha256_of_the_content() {
        assert_eq!(digest(&b"abc"[..]).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(digest(&vec![7u8; 200_000][..]).unwrap().len(), 64);
    }
}
