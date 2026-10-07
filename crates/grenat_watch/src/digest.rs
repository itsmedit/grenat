//! A file's content, told apart from another: its SHA-256, in hex, read in
//! pieces (a large file is never held in memory).

use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

/// The SHA-256 of the file at `path`, in lowercase hex.
pub fn digest(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
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
        let path = std::env::temp_dir().join(format!("grenat-watch-digest-{}", std::process::id()));
        std::fs::write(&path, "abc").unwrap();
        assert_eq!(digest(&path).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        std::fs::write(&path, vec![7u8; 200_000]).unwrap();
        assert_eq!(digest(&path).unwrap().len(), 64);
        std::fs::remove_file(&path).unwrap();
        assert!(digest(&path).is_err());
    }
}
