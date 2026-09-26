//! Loading a program: its entry file and every file it requires.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use grenat_ast::Diagnostic;
use grenat_report::Sources;

use crate::requires::requires;
use crate::resolve::{Package, Resolver, find_package};

/// Every file of a program, dependencies first.
#[derive(Debug)]
pub struct Bundle {
    pub sources: Sources,
    /// The package of the entry file, if it is in one.
    pub package: Option<Package>,
}

#[derive(Debug)]
pub enum LoadError {
    /// Errors in one file (syntax, a `require` that cannot be resolved).
    Diagnostics {
        sources: Sources,
        diagnostics: Vec<Diagnostic>,
    },
    Message(String),
}

/// Loads `entry` and what it requires. `update` fetches the latest commits
/// of git dependencies instead of the locked ones.
pub fn load(entry: &Path, update: bool) -> Result<Bundle, LoadError> {
    load_with(entry, update, &HashMap::new())
}

/// As [`load`], the texts of `overlay` (by absolute path) replacing the
/// files on disk: an editor's unsaved buffers.
pub fn load_with(entry: &Path, update: bool, overlay: &HashMap<PathBuf, String>) -> Result<Bundle, LoadError> {
    let package = find_package(entry).map_err(LoadError::Message)?;
    let mut loader = Loader {
        resolver: Resolver::new(package.as_ref(), update).map_err(LoadError::Message)?,
        seen: HashSet::new(),
        files: Vec::new(),
        overlay,
    };
    loader.visit(entry, entry.to_string_lossy().into_owned())?;
    loader.resolver.save_lock().map_err(LoadError::Message)?;
    // the application's models (`config/models.yml`), declared first
    let root = package.as_ref().map(|p| p.root.clone()).or_else(|| entry.parent().map(Path::to_path_buf));
    if let Some(root) = root {
        let file = root.join(grenat_config::models::FILE);
        if let Ok(yaml) = std::fs::read_to_string(&file) {
            let shown = file.to_string_lossy().into_owned();
            let declarations =
                grenat_config::models::declarations(&yaml).map_err(|e| LoadError::Message(format!("{shown}: {e}")))?;
            loader.files.insert(0, (shown, declarations));
        }
    }
    let sources = Sources::join(loader.files.iter().map(|(path, text)| (path.as_str(), text.as_str())));
    Ok(Bundle { sources, package })
}

struct Loader<'o> {
    resolver: Resolver,
    overlay: &'o HashMap<PathBuf, String>,
    /// Canonical paths of the files loaded or being loaded.
    seen: HashSet<PathBuf>,
    /// (displayed path, text), in load order.
    files: Vec<(String, String)>,
}

impl Loader<'_> {
    fn visit(&mut self, path: &Path, shown: String) -> Result<(), LoadError> {
        let unreadable = |e: std::io::Error| LoadError::Message(format!("cannot read {shown}: {e}"));
        let canonical = match path.canonicalize() {
            Ok(canonical) => canonical,
            // an editor's buffer not saved yet
            Err(_) if self.overlay.contains_key(path) => path.to_path_buf(),
            Err(e) => return Err(unreadable(e)),
        };
        if !self.seen.insert(canonical.clone()) {
            // loaded already, or being loaded (a cycle): once is enough
            return Ok(());
        }
        let text = match self.overlay.get(&canonical) {
            Some(text) => text.clone(),
            None => std::fs::read_to_string(&canonical).map_err(unreadable)?,
        };
        let parsed = grenat_parser::parse(&text);
        let fail = |diagnostics| LoadError::Diagnostics { sources: Sources::single(&shown, &text), diagnostics };
        if !parsed.diagnostics.is_empty() {
            return Err(fail(parsed.diagnostics));
        }
        for require in requires(&parsed.program) {
            let target = require.target.map_err(|e| e.to_string()).and_then(|t| self.target(&canonical, &t));
            match target {
                Ok(file) => {
                    let shown = display(&file);
                    self.visit(&file, shown)?;
                }
                Err(message) => return Err(fail(vec![Diagnostic::new(require.span, message)])),
            }
        }
        self.files.push((shown, text));
        Ok(())
    }

    /// The file `require "<target>"` in `file` loads.
    fn target(&mut self, file: &Path, target: &str) -> Result<PathBuf, String> {
        let path = if target.starts_with("./") || target.starts_with("../") {
            with_extension(file.parent().unwrap_or(Path::new(".")).join(target))
        } else {
            self.package_file(file, target)?
        };
        if path.is_file() || self.overlay.contains_key(&path) {
            Ok(path)
        } else {
            Err(format!("cannot find `{target}` (no file {})", display(&path)))
        }
    }

    /// `require "name"` or `"name/sub"`: a file of the package `name`, a
    /// dependency of the requiring file's package (or that package itself).
    fn package_file(&mut self, file: &Path, target: &str) -> Result<PathBuf, String> {
        let (name, rest) = target.split_once('/').map_or((target, None), |(n, r)| (n, Some(r)));
        let package = self.resolver.package_of(file)?.ok_or_else(|| {
            format!(
                "`require \"{target}\"` needs a dependency named `{name}` in a {}, or a path: `./{target}`",
                crate::MANIFEST
            )
        })?;
        let owner = if name == package.manifest.name {
            package
        } else if let Some(dep) = package.manifest.dependency(name).cloned() {
            self.resolver.dependency(&package, &dep)?
        } else if let Some(facet) = self.resolver.facet(name)? {
            facet
        } else {
            return Err(format!(
                "unknown package `{name}`: add `facet \"{name}\"` to the Facetfile (or it to `[dependencies]` in {})",
                display(&package.root.join(crate::MANIFEST))
            ));
        };
        Ok(match rest {
            Some(rest) => with_extension(owner.root.join("src").join(rest)),
            None => owner.lib(),
        })
    }
}

/// `path`, with `.grn` if it has no extension.
fn with_extension(path: PathBuf) -> PathBuf {
    if path.extension().is_some() { path } else { path.with_extension("grn") }
}

/// A path as shown in diagnostics: relative to the current directory when inside it.
fn display(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let cwd = std::env::current_dir().ok().and_then(|d| d.canonicalize().ok());
    match cwd.and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf)) {
        Some(relative) => relative.to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}
