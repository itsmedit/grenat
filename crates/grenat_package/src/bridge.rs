//! Bridge facets in a package: installing them (their server describes
//! itself, their declarations are written), and finding them for a run.
//!
//! A facet ships Ruby or Python functions with a `[bridge]` section in its
//! `grenat.toml`, served by a process of its own. That code runs outside
//! Grenat's sandbox, so the application must trust it (`facet "texts",
//! bridge: true`, see [`crate::trust`]) before it is started or its
//! declarations loaded. A package's own bridge is its author's, trusted.

use std::path::{Path, PathBuf};

use grenat_bridge::layout::Layout;
use grenat_bridge::{Installed, install as install_bridge};

use crate::facets;
use crate::manifest::Manifest;
use crate::trust::{Foreign, Trust};

/// Installs the bridge of the package in `dir` (named `name`), if it has
/// one: its manifest and declarations, checked to parse.
pub fn install(name: &str, dir: &Path) -> Result<Option<Installed>, String> {
    let Some(spec) = Manifest::load(dir)?.bridge else { return Ok(None) };
    let installed = install_bridge(name, dir, &spec).map_err(|e| format!("facet `{name}`: bridge: {e}"))?;
    let declarations = Layout::new(dir).declarations();
    let text =
        std::fs::read_to_string(&declarations).map_err(|e| format!("cannot read {}: {e}", declarations.display()))?;
    if let Some(d) = grenat_parser::parse(&text).diagnostics.first() {
        return Err(format!("facet `{name}`: the declarations of its bridge do not parse: {}", d.message));
    }
    Ok(Some(installed))
}

/// The declarations file of the bridge package `name` in `dir`, which must be installed.
pub(crate) fn declarations(name: &str, dir: &Path) -> Result<PathBuf, String> {
    let path = Layout::new(dir).declarations();
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("the bridge of facet `{name}` is not installed: run `setter install`"))
    }
}

/// The bridges a program of the package in `root` may call: its own, and
/// those of the installed facets it trusts.
pub fn bridges(root: &Path) -> Result<Vec<Installed>, String> {
    let mut bridges = Vec::new();
    if let Ok(manifest) = Manifest::load(root)
        && let Some(spec) = &manifest.bridge
        && Layout::new(root).manifest().is_file()
    {
        bridges.push(Installed::read(&manifest.name, root, spec)?);
    }
    let trust = Trust::load(root)?;
    for facet in facets::installed(root)? {
        let dir = if facet.dir.is_absolute() { facet.dir.clone() } else { root.join(&facet.dir) };
        let Some(spec) = Manifest::load(&dir).ok().and_then(|m| m.bridge) else { continue };
        if trust.allows(&facet.name, Foreign::Bridge) && Layout::new(&dir).manifest().is_file() {
            bridges.push(Installed::read(&facet.name, &dir, &spec)?);
        }
    }
    Ok(bridges)
}

/// How many functions the bridge of `name`, installed in `dir`, exports, and its command.
pub fn functions(name: &str, dir: &Path) -> Option<(usize, String)> {
    let spec = Manifest::load(dir).ok()?.bridge?;
    Installed::read(name, dir, &spec).ok().map(|i| (i.manifest.functions.len(), spec.line()))
}
