//! A file under a watched directory, opened only if what is opened really
//! is a regular file inside it.
//!
//! A scan's verdict on a link holds only when it is made: whoever drops
//! files may swap a file for a link leading out (`a.pdf -> /etc/passwd`)
//! between the scan and the read. So the check is made on the file
//! *opened*, not on its name: the last component is opened without
//! following a link (`O_NOFOLLOW`; a link inside is resolved first, and
//! its target opened that way), without waiting on a pipe (`O_NONBLOCK`),
//! then the descriptor must be a regular file whose real path — asked of
//! the system (`F_GETPATH` on macOS, `/proc/self/fd` on Linux) — lies
//! inside the directory. Whatever is swapped afterwards changes nothing:
//! the descriptor is what is read.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Why a file was not opened.
#[derive(Debug)]
pub enum OpenError {
    /// Never read, whatever its permissions: a link leading out, not a
    /// regular file.
    Refused(&'static str),
    Io(io::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Refused(why) => write!(f, "refused: {why}"),
            OpenError::Io(e) => e.fmt(f),
        }
    }
}

impl From<io::Error> for OpenError {
    fn from(e: io::Error) -> OpenError {
        OpenError::Io(e)
    }
}

/// What a scan says of a link leading out, and a read too.
pub const LEADING_OUT: &str = "a link leading out of the watched directory";

/// Opens `path` for reading if it is a regular file inside `inside` (a
/// directory, resolved: [`fs::canonicalize`]).
pub fn open_inside(path: &Path, inside: &Path) -> Result<File, OpenError> {
    let target = if fs::symlink_metadata(path)?.file_type().is_symlink() {
        let target = fs::canonicalize(path)?;
        if !target.starts_with(inside) {
            return Err(OpenError::Refused(LEADING_OUT));
        }
        target
    } else {
        path.to_path_buf()
    };
    let file = open_no_follow(&target)?;
    if !file.metadata()?.is_file() {
        return Err(OpenError::Refused("not a regular file"));
    }
    if !real_path(&file, &target)?.starts_with(inside) {
        return Err(OpenError::Refused(LEADING_OUT));
    }
    Ok(file)
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

/// Where the file opened really is.
#[cfg(target_os = "macos")]
fn real_path(file: &File, _: &Path) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes, NUL included
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let len = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buffer[..len])))
}

#[cfg(target_os = "linux")]
fn real_path(file: &File, path: &Path) -> io::Result<PathBuf> {
    use std::os::unix::io::AsRawFd;
    match fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())) {
        Ok(real) => Ok(real),
        // no /proc mounted
        Err(_) => same_file_at(file, path),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn real_path(file: &File, path: &Path) -> io::Result<PathBuf> {
    same_file_at(file, path)
}

/// The resolved `path`, if it still names the file opened.
#[cfg(all(unix, not(target_os = "macos")))]
fn same_file_at(file: &File, path: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let real = fs::canonicalize(path)?;
    let (opened, named) = (file.metadata()?, fs::symlink_metadata(&real)?);
    if (opened.dev(), opened.ino()) != (named.dev(), named.ino()) {
        return Err(io::Error::other("the file changed while it was opened"));
    }
    Ok(real)
}

#[cfg(not(unix))]
fn same_file_at(_: &File, path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::Read;
    use std::os::unix::fs::symlink;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("grenat-watch-open-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    fn read(path: &Path, inside: &Path) -> Result<String, String> {
        let mut text = String::new();
        let mut file = open_inside(path, inside).map_err(|e| e.to_string())?;
        file.read_to_string(&mut text).unwrap();
        Ok(text)
    }

    #[test]
    fn only_a_regular_file_inside_is_opened() {
        let dir = temp("inside");
        let inbox = dir.join("inbox");
        fs::create_dir_all(inbox.join("sub")).unwrap();
        fs::write(dir.join("secret.pdf"), "secret").unwrap();
        fs::write(inbox.join("real.pdf"), "real").unwrap();
        symlink(inbox.join("real.pdf"), inbox.join("alias.pdf")).unwrap();
        symlink(dir.join("secret.pdf"), inbox.join("escape.pdf")).unwrap();
        // a directory swapped for a link: the name looks inside, the file is not
        symlink(&dir, inbox.join("up")).unwrap();
        assert_eq!(read(&inbox.join("real.pdf"), &inbox).unwrap(), "real");
        assert_eq!(read(&inbox.join("alias.pdf"), &inbox).unwrap(), "real");
        assert_eq!(read(&inbox.join("escape.pdf"), &inbox).unwrap_err(), format!("refused: {LEADING_OUT}"));
        assert_eq!(read(&inbox.join("up/secret.pdf"), &inbox).unwrap_err(), format!("refused: {LEADING_OUT}"));
        assert_eq!(read(&inbox.join("sub"), &inbox).unwrap_err(), "refused: not a regular file");
        assert!(matches!(open_inside(&inbox.join("gone.pdf"), &inbox), Err(OpenError::Io(_))));
    }

    #[test]
    fn a_pipe_is_refused_without_waiting_for_a_writer() {
        let dir = temp("pipe");
        let pipe = dir.join("a.pdf");
        let status = std::process::Command::new("mkfifo").arg(&pipe).status().unwrap();
        assert!(status.success());
        assert_eq!(read(&pipe, &dir).unwrap_err(), "refused: not a regular file");
    }

    #[test]
    fn what_is_opened_is_read_whatever_is_swapped_afterwards() {
        let dir = temp("swap");
        let inbox = dir.join("inbox");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(dir.join("secret.pdf"), "secret").unwrap();
        fs::write(inbox.join("a.pdf"), "mine").unwrap();
        let mut file = open_inside(&inbox.join("a.pdf"), &inbox).unwrap();
        fs::remove_file(inbox.join("a.pdf")).unwrap();
        symlink(dir.join("secret.pdf"), inbox.join("a.pdf")).unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert_eq!(text, "mine");
        assert!(matches!(open_inside(&inbox.join("a.pdf"), &inbox), Err(OpenError::Refused(LEADING_OUT))));
    }
}
