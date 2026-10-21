//! Opening a file a program reads (`File.read`, `File.lines`, `Pdf.read`,
//! `Image.read`, `Audio.read`).
//!
//! A path under a folder an `on_file` watches names a file of the
//! dropper's: it may have been swapped for a link leading out of the
//! folder since it was scanned (`inbox/a.pdf -> ~/.ssh/id_ed25519`). Such a
//! path is opened by [`grenat_watch::open_inside`]: only a regular file
//! really inside the folder, checked on the file opened — what is read is
//! that file, whatever is swapped afterwards. "Under" is decided on the
//! path's text made absolute (`inbox/a.pdf`, as a handler is given it);
//! every other path is opened as it is.

use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

use grenat_watch::OpenError;

use crate::prelude::*;

impl<'p> Interp<'p> {
    /// Opens `path` for reading; one under a watched folder only if it is
    /// a regular file inside it.
    pub(crate) fn open_for_reading(&self, path: &str) -> io::Result<File> {
        let path = Path::new(path);
        let Some(target) = absolute(path) else { return File::open(path) };
        let watching = self
            .folders
            .borrow()
            .iter()
            .map(|folder| folder.glob.base().to_path_buf())
            .find(|base| absolute(base).is_some_and(|base| target.starts_with(base)));
        let Some(base) = watching else { return File::open(path) };
        let inside = std::fs::canonicalize(base)?;
        grenat_watch::open_inside(path, &inside).map_err(|e| match e {
            OpenError::Io(e) => e,
            refused => io::Error::new(io::ErrorKind::PermissionDenied, refused.to_string()),
        })
    }
}

/// `path` made absolute by its text alone (`.` and `..` taken away), as
/// what it names may change at any time.
fn absolute(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in std::path::absolute(path).ok()?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_made_absolute_by_its_text() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute(Path::new("inbox/./a.pdf")).unwrap(), cwd.join("inbox/a.pdf"));
        assert_eq!(absolute(Path::new("inbox/sub/../a.pdf")).unwrap(), cwd.join("inbox/a.pdf"));
        assert_eq!(absolute(Path::new("/x/../y")).unwrap(), PathBuf::from("/y"));
        assert!(!absolute(Path::new("inbox/../outbox/a.pdf")).unwrap().starts_with(cwd.join("inbox")));
    }
}
