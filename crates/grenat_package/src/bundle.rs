//! Loading a program: its entry file and every file it requires.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use grenat_ast::Diagnostic;
use grenat_report::Sources;

use crate::requires::requires;
use crate::resolve::{Package, Resolver, find_package};
use crate::trust::{Foreign, Trust};

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
        trust: None,
    };
    // a native or bridge facet's own programs (its tests) see its functions
    if let Some(package) = &package
        && let Some(kind) = Foreign::of(&package.manifest)
    {
        let declarations =
            foreign_declarations(kind, &package.manifest.name, &package.root).map_err(LoadError::Message)?;
        loader.visit(&declarations, display(&declarations), Origin::Generated)?;
    }
    loader.visit(entry, entry.to_string_lossy().into_owned(), Origin::Written)?;
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
    /// The facets whose code outside Grenat the root package trusts, read once.
    trust: Option<Trust>,
}

/// Who wrote a file of the program.
#[derive(Clone, Copy, PartialEq)]
enum Origin {
    /// `setter install`, from what a native or bridge facet exports: its `native def`s.
    Generated,
    Written,
}

impl Loader<'_> {
    fn visit(&mut self, path: &Path, shown: String, origin: Origin) -> Result<(), LoadError> {
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
        if origin == Origin::Written {
            let forged = hand_written_natives(&parsed.program);
            if !forged.is_empty() {
                return Err(fail(forged));
            }
        }
        for require in requires(&parsed.program) {
            let target = require.target.map_err(|e| e.to_string()).and_then(|t| self.target(&canonical, &t));
            match target {
                Ok((file, native)) => {
                    // a native or bridge facet's declarations, generated from what it exports
                    if let Some(declarations) = native {
                        let shown = display(&declarations);
                        self.visit(&declarations, shown, Origin::Generated)?;
                    }
                    let shown = display(&file);
                    self.visit(&file, shown, Origin::Written)?;
                }
                Err(message) => return Err(fail(vec![Diagnostic::new(require.span, message)])),
            }
        }
        self.files.push((shown, text));
        Ok(())
    }

    /// The file `require "<target>"` in `file` loads, and the declarations
    /// of its package if it is a native or bridge facet.
    fn target(&mut self, file: &Path, target: &str) -> Result<(PathBuf, Option<PathBuf>), String> {
        let (path, native) = if target.starts_with("./") || target.starts_with("../") {
            (with_extension(file.parent().unwrap_or(Path::new(".")).join(target)), None)
        } else {
            self.package_file(file, target)?
        };
        if path.is_file() || self.overlay.contains_key(&path) {
            Ok((path, native))
        } else {
            Err(format!("cannot find `{target}` (no file {})", display(&path)))
        }
    }

    /// `require "name"` or `"name/sub"`: a file of the package `name`, a
    /// dependency of the requiring file's package (or that package itself).
    fn package_file(&mut self, file: &Path, target: &str) -> Result<(PathBuf, Option<PathBuf>), String> {
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
        let native = self.foreign_declarations(&owner)?;
        let path = match rest {
            Some(rest) => with_extension(owner.root.join("src").join(rest)),
            None => owner.lib(),
        };
        Ok((path, native))
    }

    /// The declarations of `owner`'s native code or bridge, if it has one:
    /// only for the root package itself, or a facet the root package trusts.
    fn foreign_declarations(&mut self, owner: &Package) -> Result<Option<PathBuf>, String> {
        let Some(kind) = Foreign::of(&owner.manifest) else { return Ok(None) };
        let root = self.resolver.root().map(Path::to_path_buf);
        if root.as_deref() != Some(owner.root.as_path()) {
            let trust = match (&self.trust, &root) {
                (Some(trust), _) => trust.clone(),
                (None, Some(root)) => Trust::load(root)?,
                (None, None) => Trust::default(),
            };
            let refused =
                (!trust.allows(&owner.manifest.name, kind)).then(|| trust.refusal(&owner.manifest.name, kind));
            self.trust = Some(trust);
            if let Some(refusal) = refused {
                return Err(refusal);
            }
        }
        foreign_declarations(kind, &owner.manifest.name, &owner.root).map(Some)
    }
}

/// The declarations file of the package `name` in `dir`, as its kind of code writes it.
fn foreign_declarations(kind: Foreign, name: &str, dir: &Path) -> Result<PathBuf, String> {
    match kind {
        Foreign::Native => crate::native::declarations(name, dir),
        Foreign::Bridge => crate::bridge::declarations(name, dir),
    }
}

/// The `native def`s of a file written by hand: refused, since a `native
/// def` stands for code outside Grenat, whose effects and purity only its
/// facet's manifest can say (the interpreter checks them against it too).
fn hand_written_natives(program: &grenat_ast::Program) -> Vec<Diagnostic> {
    program
        .items
        .iter()
        .filter_map(|item| match item {
            grenat_ast::Item::Fn(def) if def.kind == grenat_ast::FnKind::Native => Some(
                Diagnostic::new(
                    def.span,
                    format!("`{}` is a `native def` written by hand: `setter install` writes them", def.name.name),
                )
                .with_help(
                    "a `native def` comes from what a native or bridge facet exports: require the facet, and run \
                     `setter install`",
                ),
            ),
            _ => None,
        })
        .collect()
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
