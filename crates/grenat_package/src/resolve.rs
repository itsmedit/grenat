//! Packages and where their dependencies are on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::git;
use crate::lock::{Lock, Locked};
use crate::manifest::{Dependency, Manifest, Source};

/// A package: a directory with a `grenat.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Package {
    pub fn load(root: &Path) -> Result<Package, String> {
        Ok(Package { root: root.to_path_buf(), manifest: Manifest::load(root)? })
    }

    pub fn main(&self) -> PathBuf {
        self.root.join(&self.manifest.main)
    }

    pub fn lib(&self) -> PathBuf {
        self.root.join(&self.manifest.lib)
    }
}

/// The package containing `path` (a file or a directory): the nearest
/// directory above it with a `grenat.toml`.
pub fn find_package(path: &Path) -> Result<Option<Package>, String> {
    let start = if path.is_dir() { path.to_path_buf() } else { path.parent().unwrap_or(Path::new(".")).to_path_buf() };
    let start = start.canonicalize().unwrap_or(start);
    for dir in start.ancestors() {
        if dir.join(crate::MANIFEST).is_file() {
            return Package::load(dir).map(Some);
        }
    }
    Ok(None)
}

/// Finds (and fetches) the dependencies of the packages of one program.
pub(crate) struct Resolver {
    /// The root package: git dependencies go to its `.grenat/deps/`, its
    /// lock file pins them.
    root: Option<PathBuf>,
    lock: Lock,
    /// Ignore the lock file: fetch the latest commits (`grenat update`).
    update: bool,
    /// Git dependencies fetched by this load: name → (url, directory).
    fetched: HashMap<String, (String, PathBuf)>,
    /// Packages by root directory.
    packages: HashMap<PathBuf, Package>,
}

impl Resolver {
    pub(crate) fn new(root: Option<&Package>, update: bool) -> Result<Resolver, String> {
        let lock = match root {
            Some(package) => Lock::load(&package.root)?,
            None => Lock::default(),
        };
        Ok(Resolver {
            root: root.map(|p| p.root.clone()),
            lock,
            update,
            fetched: HashMap::new(),
            packages: HashMap::new(),
        })
    }

    /// The package of a file, loaded once.
    pub(crate) fn package_of(&mut self, file: &Path) -> Result<Option<Package>, String> {
        let dir = file.parent().unwrap_or(Path::new("."));
        if let Some(known) = self.packages.values().filter(|p| dir.starts_with(&p.root)).max_by_key(|p| p.root.as_os_str().len())
            && !has_nearer_manifest(dir, &known.root)
        {
            return Ok(Some(known.clone()));
        }
        let package = find_package(file)?;
        if let Some(package) = &package {
            self.packages.insert(package.root.clone(), package.clone());
        }
        Ok(package)
    }

    /// Where the dependency `dep` of `package` is, fetching it if needed.
    pub(crate) fn dependency(&mut self, package: &Package, dep: &Dependency) -> Result<Package, String> {
        let root = match &dep.source {
            Source::Path(path) => {
                let root = package.root.join(path);
                root.canonicalize().map_err(|e| format!("dependency `{}`: cannot find {}: {e}", dep.name, root.display()))?
            }
            Source::Git { url, reference } => self.git(dep, url, reference)?,
        };
        if !root.join(crate::MANIFEST).is_file() {
            return Err(format!("dependency `{}`: no {} in {}", dep.name, crate::MANIFEST, root.display()));
        }
        let loaded = match self.packages.get(&root) {
            Some(p) => p.clone(),
            None => Package::load(&root)?,
        };
        self.packages.insert(root, loaded.clone());
        Ok(loaded)
    }

    fn git(&mut self, dep: &Dependency, url: &str, reference: &crate::Reference) -> Result<PathBuf, String> {
        if let Some((fetched_url, dir)) = self.fetched.get(&dep.name) {
            return if fetched_url == url {
                Ok(dir.clone())
            } else {
                Err(format!("two dependencies are named `{}`: {fetched_url} and {url}", dep.name))
            };
        }
        let Some(root) = &self.root else {
            return Err(format!("dependency `{}`: git dependencies need a root package", dep.name));
        };
        let dir = root.join(".grenat").join("deps").join(&dep.name);
        let described = reference.describe();
        let locked = self.lock.entries.get(&dep.name).filter(|l| !self.update && l.url == url && l.reference == described);
        let commit = match locked {
            Some(l) if dir.is_dir() && git::head(&dir).is_ok_and(|head| head == l.commit) => l.commit.clone(),
            Some(l) => git::fetch(url, reference, Some(&l.commit), &dir)?,
            None => git::fetch(url, reference, None, &dir)?,
        };
        let entry = Locked { url: url.to_string(), reference: described, commit };
        self.lock.entries.insert(dep.name.clone(), entry);
        self.fetched.insert(dep.name.clone(), (url.to_string(), dir.clone()));
        Ok(dir)
    }

    /// Saves the lock file. Entries this program did not need are kept: the
    /// package's other programs (its tests…) may need them.
    pub(crate) fn save_lock(&mut self) -> Result<(), String> {
        let Some(root) = &self.root else { return Ok(()) };
        if self.lock.entries.is_empty() && !root.join(crate::LOCK).exists() {
            return Ok(());
        }
        self.lock.save(root)
    }
}

/// Whether a directory between `dir` and `root` (excluded) has its own manifest.
fn has_nearer_manifest(dir: &Path, root: &Path) -> bool {
    dir.ancestors().take_while(|d| *d != root).any(|d| d.join(crate::MANIFEST).is_file())
}
