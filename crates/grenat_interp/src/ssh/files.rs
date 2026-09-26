//! The files of a server, as SFTP gives them: one interface for a real SFTP
//! session and for a test double. Paths are the server's; `local` ones are
//! this machine's.

use std::path::Path;

use grenat_ssh::{Entry, Error, Sftp};

pub(crate) trait RemoteFiles {
    /// The entries of `dir`, by name.
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, Error>;
    fn read(&mut self, path: &str) -> Result<Vec<u8>, Error>;
    /// Creates or replaces the file at `path`.
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error>;
    fn upload(&mut self, local: &Path, remote: &str) -> Result<(), Error>;
    fn download(&mut self, remote: &str, local: &Path) -> Result<(), Error>;
    /// Removes a file or an empty directory.
    fn remove(&mut self, path: &str) -> Result<(), Error>;
    fn mkdir(&mut self, path: &str) -> Result<(), Error>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), Error>;
    fn exists(&mut self, path: &str) -> Result<bool, Error>;
}

impl RemoteFiles for Sftp {
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, Error> {
        Sftp::list(self, dir)
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, Error> {
        Sftp::read(self, path)
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        Sftp::write(self, path, bytes)
    }

    fn upload(&mut self, local: &Path, remote: &str) -> Result<(), Error> {
        Sftp::upload(self, local, remote)
    }

    fn download(&mut self, remote: &str, local: &Path) -> Result<(), Error> {
        Sftp::download(self, remote, local)
    }

    fn remove(&mut self, path: &str) -> Result<(), Error> {
        Sftp::remove(self, path)
    }

    fn mkdir(&mut self, path: &str) -> Result<(), Error> {
        Sftp::mkdir(self, path)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        Sftp::rename(self, from, to)
    }

    fn exists(&mut self, path: &str) -> Result<bool, Error> {
        Sftp::exists(self, path)
    }
}
