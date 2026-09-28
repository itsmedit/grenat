//! Native facets in a package: which the application trusts, building them
//! at install, and finding their libraries for a run.
//!
//! A facet ships Rust code with a `[native]` section in its `grenat.toml`.
//! That code runs outside Grenat's sandbox — no capability, no taint
//! tracking inside it — so the application must say it trusts it, in its
//! own `Facetfile`: `facet "sheets", "~> 0.1", native: true`. Without it,
//! neither `setter install` (which would build, and so run, its code) nor
//! loading a program that requires it goes further. A package's own native
//! part is its author's, trusted.

use std::collections::HashSet;
use std::path::Path;

use grenat_native::layout::Layout;
use grenat_native::{Installed, install};

use crate::facetfile::Facetfile;
use crate::facets;
use crate::manifest::Manifest;

/// Why a native facet the application does not trust is refused, and how to trust it.
pub fn refusal(name: &str) -> String {
    format!(
        "facet `{name}` ships native code (Rust), which runs outside Grenat's sandbox: \
         trust it with `facet \"{name}\", native: true` in the Facetfile"
    )
}

/// The facets whose native code the package in `root` trusts.
pub fn trusted(root: &Path) -> Result<HashSet<String>, String> {
    Ok(Facetfile::load(root)?
        .map(|file| file.facets.into_iter().filter(|f| f.native).map(|f| f.name).collect())
        .unwrap_or_default())
}

/// Refuses the native facets among `facets` (name, directory) that `root` does not trust.
pub fn check_trust(root: &Path, facets: &[(String, std::path::PathBuf)]) -> Result<(), String> {
    let trusted = trusted(root)?;
    for (name, dir) in facets {
        if Manifest::load(dir)?.native.is_some() && !trusted.contains(name) {
            return Err(refusal(name));
        }
    }
    Ok(())
}

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
    let trusted = trusted(root)?;
    for facet in facets::installed(root)? {
        let dir = if facet.dir.is_absolute() { facet.dir.clone() } else { root.join(&facet.dir) };
        let native = Manifest::load(&dir).is_ok_and(|m| m.native.is_some());
        if native && trusted.contains(&facet.name) && Layout::new(&facet.name, &dir).manifest().is_file() {
            libraries.push(Installed::read(&facet.name, &dir)?);
        }
    }
    Ok(libraries)
}

/// How many functions the native part of `name`, installed in `dir`, exports.
pub fn functions(name: &str, dir: &Path) -> Option<usize> {
    Installed::read(name, dir).ok().map(|i| i.manifest.functions.len())
}
