//! Native facets in a package: building them at install, and finding
//! their libraries for a run.
//!
//! A facet ships Rust code with a `[native]` section in its `grenat.toml`.
//! That code runs outside Grenat's sandbox, so the application must trust
//! it (`facet "sheets", "~> 0.1", native: true`, see [`crate::trust`])
//! before it is built or loaded. A package's own native part is its
//! author's, trusted.

use std::path::Path;

use grenat_native::layout::Layout;
use grenat_native::{Installed, install};

use crate::facets;
use crate::manifest::Manifest;
use crate::trust::{Foreign, Trust};

/// Builds the native part of the package in `dir` (named `name`), if it has
/// one: its library, manifest and declarations, checked to parse.
pub fn build(name: &str, dir: &Path) -> Result<Option<Installed>, String> {
    let Some(native) = Manifest::load(dir)?.native else { return Ok(None) };
    let installed = install(name, dir, &native.path, None).map_err(|e| format!("facet `{name}`: native code: {e}"))?;
    let declarations = Layout::new(name, dir).declarations();
    let text =
        std::fs::read_to_string(&declarations).map_err(|e| format!("cannot read {}: {e}", declarations.display()))?;
    if let Some(d) = grenat_parser::parse(&text).diagnostics.first() {
        return Err(format!("facet `{name}`: the declarations of its library do not parse: {}", d.message));
    }
    Ok(Some(installed))
}

/// The declarations file of the native package `name` in `dir`, which must be built.
pub(crate) fn declarations(name: &str, dir: &Path) -> Result<std::path::PathBuf, String> {
    let path = Layout::new(name, dir).declarations();
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("the native part of facet `{name}` is not built: run `setter install`"))
    }
}

/// The native libraries a program of the package in `root` may call: its
/// own, and those of the installed facets it trusts.
pub fn libraries(root: &Path) -> Result<Vec<Installed>, String> {
    let mut libraries = Vec::new();
    if let Ok(manifest) = Manifest::load(root)
        && manifest.native.is_some()
        && Layout::new(&manifest.name, root).manifest().is_file()
    {
        libraries.push(Installed::read(&manifest.name, root)?);
    }
    let trust = Trust::load(root)?;
    for facet in facets::installed(root)? {
        let dir = if facet.dir.is_absolute() { facet.dir.clone() } else { root.join(&facet.dir) };
        let native = Manifest::load(&dir).is_ok_and(|m| m.native.is_some());
        if native && trust.allows(&facet.name, Foreign::Native) && Layout::new(&facet.name, &dir).manifest().is_file() {
            libraries.push(Installed::read(&facet.name, &dir)?);
        }
    }
    Ok(libraries)
}

/// How many functions the native part of `name`, installed in `dir`, exports.
pub fn functions(name: &str, dir: &Path) -> Option<usize> {
    Installed::read(name, dir).ok().map(|i| i.manifest.functions.len())
}
