//! Scratch files for the tests: a directory of their own per process.

use std::path::PathBuf;

/// A path named `name` in this process's scratch directory (created).
pub(crate) fn path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sheets-native-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir.join(name)
}

/// The path as the exported functions take it.
pub(crate) fn text(name: &str) -> String {
    path(name).to_string_lossy().into_owned()
}
