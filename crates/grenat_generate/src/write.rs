//! Writing what a generator made: new files only (nothing is overwritten),
//! and the changes reported.

use std::path::{Path, PathBuf};

/// A file a generator wrote, relative to the application.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Created(PathBuf),
    Updated(PathBuf),
}

pub(crate) struct Writer<'a> {
    root: &'a Path,
    pub changes: Vec<Change>,
}

impl<'a> Writer<'a> {
    pub fn new(root: &'a Path) -> Writer<'a> {
        Writer { root, changes: Vec::new() }
    }

    /// Fails if `path` exists: a generator never overwrites.
    pub fn check_new(&self, path: &str) -> Result<(), String> {
        if self.root.join(path).exists() {
            return Err(format!("{path} already exists"));
        }
        Ok(())
    }

    pub fn create(&mut self, path: &str, text: &str) -> Result<(), String> {
        self.check_new(path)?;
        self.write(path, text)?;
        self.changes.push(Change::Created(path.into()));
        Ok(())
    }

    pub fn update(&mut self, path: &str, text: &str) -> Result<(), String> {
        self.write(path, text)?;
        self.changes.push(Change::Updated(path.into()));
        Ok(())
    }

    fn write(&self, path: &str, text: &str) -> Result<(), String> {
        let full = self.root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&full, text).map_err(|e| format!("cannot write {}: {e}", full.display()))
    }
}
