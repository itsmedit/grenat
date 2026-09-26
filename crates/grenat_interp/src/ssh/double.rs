//! `mock_ssh`: a server that exists only in a test. Commands are answered
//! from a table (a trailing `*` matches a prefix of the command line), files
//! are kept in memory, directories are those holding files or made by
//! `mkdir`. A command it was not told about is an error, as a missing file is.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use grenat_ssh::{Entry, Error, ErrorKind, Output};

use super::files::RemoteFiles;

pub(crate) struct Double {
    /// `user@host`, for messages.
    label: String,
    /// Command line pattern → what the command does.
    commands: Vec<(String, Output)>,
    files: BTreeMap<String, Vec<u8>>,
    /// Directories made by `mkdir` (others exist because files are in them).
    dirs: BTreeSet<String>,
}

impl Double {
    pub(crate) fn new(label: &str, commands: Vec<(String, Output)>, files: Vec<(String, Vec<u8>)>) -> Double {
        let files = files.into_iter().map(|(path, bytes)| (normalize(&path), bytes)).collect();
        Double { label: label.to_string(), commands, files, dirs: BTreeSet::new() }
    }

    /// What `argv` does: the last matching entry of the table says.
    pub(crate) fn run(&self, argv: &[String]) -> Result<Output, Error> {
        let line = argv.join(" ");
        let found = self.commands.iter().rev().find(|(pattern, _)| matches(pattern, &line));
        found.map(|(_, output)| output.clone()).ok_or_else(|| {
            Error::new(ErrorKind::Command, format!("the mock of `{}` has no command `{line}`", self.label))
        })
    }

    fn is_dir(&self, path: &str) -> bool {
        matches!(path, "/" | ".")
            || self.dirs.contains(path)
            || self.files.keys().chain(&self.dirs).any(|p| under(path, p).is_some())
    }
}

impl RemoteFiles for Double {
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, Error> {
        let dir = normalize(dir);
        if !self.is_dir(&dir) {
            let reason = if self.files.contains_key(&dir) { "not a directory" } else { NOT_FOUND };
            return Err(failed(format!("list `{dir}`"), reason));
        }
        let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
        let paths = self.files.iter().map(|(p, bytes)| (p, bytes.len() as u64, false));
        for (path, size, made_dir) in paths.chain(self.dirs.iter().map(|p| (p, 0, true))) {
            let Some(rest) = under(&dir, path) else { continue };
            let (name, nested) = match rest.split_once('/') {
                Some((name, _)) => (name, true),
                None => (rest, made_dir),
            };
            let size = if nested { 0 } else { size };
            entries.insert(name.to_string(), Entry { name: name.to_string(), size, is_dir: nested, modified: None });
        }
        Ok(entries.into_values().collect())
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, Error> {
        let path = normalize(path);
        match self.files.get(&path) {
            Some(bytes) => Ok(bytes.clone()),
            None if self.is_dir(&path) => Err(failed(format!("read `{path}`"), "it is a directory")),
            None => Err(failed(format!("read `{path}`"), NOT_FOUND)),
        }
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        let path = normalize(path);
        if self.is_dir(&path) {
            return Err(failed(format!("write `{path}`"), "it is a directory"));
        }
        self.files.insert(path, bytes.to_vec());
        Ok(())
    }

    fn upload(&mut self, local: &Path, remote: &str) -> Result<(), Error> {
        let bytes = std::fs::read(local).map_err(|e| local_failed("read", local, e))?;
        self.write(remote, &bytes)
    }

    fn download(&mut self, remote: &str, local: &Path) -> Result<(), Error> {
        let bytes = self.read(remote)?;
        std::fs::write(local, bytes).map_err(|e| local_failed("write", local, e))
    }

    fn remove(&mut self, path: &str) -> Result<(), Error> {
        let path = normalize(path);
        if self.files.remove(&path).is_some() {
            return Ok(());
        }
        if !self.is_dir(&path) || matches!(path.as_str(), "/" | ".") {
            return Err(failed(format!("remove `{path}`"), NOT_FOUND));
        }
        if self.files.keys().chain(&self.dirs).any(|p| under(&path, p).is_some()) {
            return Err(failed(format!("remove `{path}`"), "the directory is not empty"));
        }
        self.dirs.remove(&path);
        Ok(())
    }

    fn mkdir(&mut self, path: &str) -> Result<(), Error> {
        let path = normalize(path);
        if self.exists(&path)? {
            return Err(failed(format!("create the directory `{path}`"), "it exists"));
        }
        self.dirs.insert(path);
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let (from, to) = (normalize(from), normalize(to));
        let what = format!("rename `{from}` to `{to}`");
        if !self.exists(&from)? || matches!(from.as_str(), "/" | ".") {
            return Err(failed(what, NOT_FOUND));
        }
        if self.exists(&to)? {
            return Err(failed(what, &format!("`{to}` exists")));
        }
        let moved = |path: &str| {
            if path == from { Some(to.clone()) } else { under(&from, path).map(|rest| format!("{to}/{rest}")) }
        };
        self.files = std::mem::take(&mut self.files)
            .into_iter()
            .map(|(path, bytes)| (moved(&path).unwrap_or(path), bytes))
            .collect();
        self.dirs = std::mem::take(&mut self.dirs).into_iter().map(|path| moved(&path).unwrap_or(path)).collect();
        Ok(())
    }

    fn exists(&mut self, path: &str) -> Result<bool, Error> {
        let path = normalize(path);
        Ok(self.files.contains_key(&path) || self.is_dir(&path))
    }
}

const NOT_FOUND: &str = "no such file or directory";

/// Whether `line` is the command `pattern` stands for.
fn matches(pattern: &str, line: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => line.starts_with(prefix),
        None => line == pattern,
    }
}

/// `a/b/` → `a/b`; `/` stays; empty is `.`.
fn normalize(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match (trimmed, path) {
        ("", "") => ".".into(),
        ("", _) => "/".into(),
        (trimmed, _) => trimmed.to_string(),
    }
}

/// What follows `dir/` in `path`, when `path` is inside `dir`.
fn under<'a>(dir: &str, path: &'a str) -> Option<&'a str> {
    let rest = match dir {
        "/" => path.strip_prefix('/')?,
        "." => Some(path).filter(|p| !p.starts_with('/'))?,
        dir => path.strip_prefix(dir)?.strip_prefix('/')?,
    };
    Some(rest).filter(|rest| !rest.is_empty())
}

fn failed(what: String, reason: &str) -> Error {
    Error::new(ErrorKind::Sftp, format!("cannot {what}: {reason}"))
}

fn local_failed(action: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(ErrorKind::Sftp, format!("cannot {action} the local file `{}`: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn double() -> Double {
        let output = Output { status: Some(0), stdout: b"done".to_vec(), ..Output::default() };
        let files = vec![("/srv/app/config.yml".into(), b"port: 80".to_vec()), ("notes.txt".into(), b"hi".to_vec())];
        Double::new("deploy@api.acme.com", vec![("systemctl restart*".into(), output)], files)
    }

    fn names(entries: &[Entry]) -> Vec<(&str, bool)> {
        entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect()
    }

    #[test]
    fn commands_from_the_table_only() {
        let d = double();
        let argv = |line: &str| line.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(d.run(&argv("systemctl restart shop")).unwrap().stdout, b"done");
        let e = d.run(&argv("rm -rf /")).unwrap_err();
        assert_eq!(
            (e.kind(), e.message()),
            (ErrorKind::Command, "the mock of `deploy@api.acme.com` has no command `rm -rf /`")
        );
    }

    #[test]
    fn files_and_directories() {
        let mut d = double();
        assert_eq!(names(&d.list("/").unwrap()), [("srv", true)]);
        assert_eq!(names(&d.list("/srv/app/").unwrap()), [("config.yml", false)]);
        assert_eq!(d.list("/srv/app").unwrap()[0].size, 8);
        assert_eq!(names(&d.list(".").unwrap()), [("notes.txt", false)]);
        assert_eq!(d.list("/nowhere").unwrap_err().message(), "cannot list `/nowhere`: no such file or directory");
        assert_eq!(d.list("notes.txt").unwrap_err().message(), "cannot list `notes.txt`: not a directory");
        assert_eq!(d.read("/srv/app/config.yml").unwrap(), b"port: 80");
        assert_eq!(d.read("/srv").unwrap_err().message(), "cannot read `/srv`: it is a directory");
        d.write("/srv/app/new", b"x").unwrap();
        assert!(d.exists("/srv/app/new").unwrap() && d.exists("/srv").unwrap() && !d.exists("/etc").unwrap());
        d.mkdir("/srv/logs").unwrap();
        assert_eq!(names(&d.list("/srv").unwrap()), [("app", true), ("logs", true)]);
        assert!(d.mkdir("/srv/logs").unwrap_err().message().ends_with("it exists"));
        d.remove("/srv/logs").unwrap();
        assert_eq!(d.remove("/srv/app").unwrap_err().message(), "cannot remove `/srv/app`: the directory is not empty");
        d.remove("/srv/app/new").unwrap();
        assert_eq!(
            d.remove("/srv/app/new").unwrap_err().message(),
            "cannot remove `/srv/app/new`: no such file or directory"
        );
        assert!(d.write("/srv", b"x").is_err());
    }

    #[test]
    fn renaming_moves_what_a_directory_holds() {
        let mut d = double();
        d.rename("/srv/app", "/srv/old").unwrap();
        assert_eq!(d.read("/srv/old/config.yml").unwrap(), b"port: 80");
        assert!(!d.exists("/srv/app").unwrap());
        d.rename("notes.txt", "n.txt").unwrap();
        assert_eq!(d.read("n.txt").unwrap(), b"hi");
        assert!(d.rename("missing", "x").unwrap_err().message().ends_with(NOT_FOUND));
        assert_eq!(
            d.rename("n.txt", "/srv/old").unwrap_err().message(),
            "cannot rename `n.txt` to `/srv/old`: `/srv/old` exists"
        );
    }

    #[test]
    fn transfers_use_local_files() {
        let mut d = double();
        let dir = std::env::temp_dir().join(format!("grenat-ssh-double-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        d.download("/srv/app/config.yml", &dir.join("config.yml")).unwrap();
        assert_eq!(std::fs::read(dir.join("config.yml")).unwrap(), b"port: 80");
        d.upload(&dir.join("config.yml"), "/srv/copy.yml").unwrap();
        assert_eq!(d.read("/srv/copy.yml").unwrap(), b"port: 80");
        let e = d.upload(&dir.join("absent"), "/x").unwrap_err();
        assert!(e.message().starts_with("cannot read the local file"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
