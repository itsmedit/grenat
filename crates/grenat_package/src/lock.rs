//! `grenat.lock`: the exact commit of every git dependency, so that a
//! program builds the same everywhere until `grenat update`.

use std::collections::BTreeMap;
use std::path::Path;

use toml::{Table, Value};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lock {
    /// Dependency name → (url, reference described, commit).
    pub entries: BTreeMap<String, Locked>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Locked {
    pub url: String,
    pub reference: String,
    pub commit: String,
}

impl Lock {
    /// The lock file of the root package in `dir` (empty if there is none).
    pub fn load(dir: &Path) -> Result<Lock, String> {
        let path = dir.join(crate::LOCK);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Lock::default()),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        Lock::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Lock, String> {
        let table: Table = text.parse().map_err(|e: toml::de::Error| e.message().to_string())?;
        let mut lock = Lock::default();
        let packages = table.get("package").and_then(Value::as_array).cloned().unwrap_or_default();
        for package in packages {
            let get = |key: &str| package.get(key).and_then(Value::as_str).map(str::to_string);
            match (get("name"), get("git"), get("reference"), get("commit")) {
                (Some(name), Some(url), Some(reference), Some(commit)) => {
                    lock.entries.insert(name, Locked { url, reference, commit });
                }
                _ => return Err("each `[[package]]` needs `name`, `git`, `reference` and `commit`".into()),
            }
        }
        Ok(lock)
    }

    pub fn render(&self) -> String {
        let mut text = String::from("# Written by grenat: the exact commit of each git dependency.\n");
        for (name, locked) in &self.entries {
            text.push_str(&format!(
                "\n[[package]]\nname = {}\ngit = {}\nreference = {}\ncommit = {}\n",
                quote(name),
                quote(&locked.url),
                quote(&locked.reference),
                quote(&locked.commit)
            ));
        }
        text
    }

    /// Writes the lock file if it changed.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let path = dir.join(crate::LOCK);
        let text = self.render();
        if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
            return Ok(());
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

fn quote(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}
