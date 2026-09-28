//! Installing a facet's native part: its library (prebuilt for this
//! platform, else built), copied into the facet ([`Layout`]), loaded once
//! to read its manifest, and the declarations it stands for written next
//! to it.

use std::path::{Path, PathBuf};

use grenat_ext::manifest::Manifest;

use crate::declarations::declarations;
use crate::layout::{Layout, prebuilt};
use crate::library::Library;

/// A facet's native part, as installed.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub facet: String,
    pub library: PathBuf,
    pub manifest: Manifest,
}

impl Installed {
    /// The native part installed in the facet `facet`'s directory `dir`
    /// (what `setter install` wrote there).
    pub fn read(facet: &str, dir: &Path) -> Result<Installed, String> {
        let layout = Layout::new(facet, dir);
        let path = layout.manifest();
        let text = std::fs::read_to_string(&path)
            .map_err(|_| format!("the native part of facet `{facet}` is not built: run `setter install`"))?;
        let manifest = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Installed { facet: facet.to_string(), library: layout.library(), manifest })
    }
}

/// Installs the native part of the facet `facet` in `dir`, whose crate is
/// `crate_path` (relative to `dir`); `target_dir` overrides cargo's.
pub fn install(facet: &str, dir: &Path, crate_path: &Path, target_dir: Option<&Path>) -> Result<Installed, String> {
    let crate_dir = dir.join(crate_path);
    let shipped = prebuilt(facet, &crate_dir);
    let built = if shipped.is_file() { shipped } else { crate::build::build(&crate_dir, target_dir)? };
    let layout = Layout::new(facet, dir);
    let library = layout.library();
    copy(&built, &library)?;
    let manifest = Library::open(&library)?.manifest.clone();
    let text = declarations(facet, &manifest)?;
    let manifest_json = serde_json::to_string_pretty(&manifest).expect("a manifest is JSON");
    write(&layout.manifest(), &manifest_json)?;
    write(&layout.declarations(), &text)?;
    Ok(Installed { facet: facet.to_string(), library, manifest })
}

/// Copies the library through a new file renamed into place: a library
/// already loaded (mapped) from `to` is never overwritten in place.
fn copy(from: &Path, to: &Path) -> Result<(), String> {
    let dir = to.parent().expect("a file in a directory");
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let partial = to.with_extension("partial");
    std::fs::copy(from, &partial).map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))?;
    std::fs::rename(&partial, to).map_err(|e| format!("cannot write {}: {e}", to.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}
