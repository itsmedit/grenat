//! Scratch files for the tests: a directory of its own per test, removed
//! when the test ends, whether it passes or fails.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A test's scratch directory, removed when dropped.
pub(crate) struct Dir {
    path: PathBuf,
}

impl Dir {
    /// A new, empty directory, distinct from every other test's.
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("sheets-native-tests-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Dir { path }
    }

    /// The path of the file `name` in the directory, as the exported
    /// functions take it.
    pub(crate) fn text(&self, name: &str) -> String {
        self.path.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_directory_is_its_own_and_removed_when_dropped() {
        let (one, two) = (Dir::new(), Dir::new());
        assert_ne!(one.text("x"), two.text("x"));
        std::fs::write(one.text("x"), "x").unwrap();
        let path = one.path.clone();
        assert!(path.is_dir());
        drop(one);
        assert!(!path.exists());
        assert!(two.path.is_dir());
    }
}
