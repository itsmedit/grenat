//! Which facets the application trusts with code that runs outside
//! Grenat's sandbox — no capability checked inside it, no taint tracked —
//! in its own `Facetfile`:
//!
//! ```ruby
//! facet "sheets", "~> 0.1", native: true   # Rust code, loaded into Grenat
//! facet "texts", "~> 0.2", bridge: true    # Ruby or Python code, in a process
//! ```
//!
//! Without it, neither `setter install` (which would build or start that
//! code) nor loading a program that requires the facet goes further. A
//! package's own code is its author's, trusted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::facetfile::Facetfile;
use crate::manifest::Manifest;

/// Code a facet ships that runs outside Grenat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foreign {
    /// `[native]`: a Rust library (see `grenat_native`).
    Native,
    /// `[bridge]`: a process serving Ruby or Python functions (see `grenat_bridge`).
    Bridge,
}

impl Foreign {
    /// What the package in `dir` ships, if anything.
    pub fn of(manifest: &Manifest) -> Option<Foreign> {
        match (&manifest.native, &manifest.bridge) {
            (Some(_), _) => Some(Foreign::Native),
            (None, Some(_)) => Some(Foreign::Bridge),
            (None, None) => None,
        }
    }

    /// The `Facetfile` option that trusts it.
    pub fn option(self) -> &'static str {
        match self {
            Foreign::Native => "native",
            Foreign::Bridge => "bridge",
        }
    }

    fn what(self) -> &'static str {
        match self {
            Foreign::Native => "native code (Rust)",
            Foreign::Bridge => "a bridge (Ruby or Python code, run as a process)",
        }
    }
}

/// The facets the application trusts, by kind of code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trust {
    native: HashSet<String>,
    bridge: HashSet<String>,
    /// Each facet's declaration, as written in the `Facetfile`.
    written: HashMap<String, String>,
}

impl Trust {
    /// What the `Facetfile` of the package in `root` trusts (nothing without one).
    pub fn load(root: &Path) -> Result<Trust, String> {
        let mut trust = Trust::default();
        for facet in Facetfile::load(root)?.map(|file| file.facets).unwrap_or_default() {
            if facet.native {
                trust.native.insert(facet.name.clone());
            }
            if facet.bridge {
                trust.bridge.insert(facet.name.clone());
            }
            trust.written.insert(facet.name, facet.written);
        }
        Ok(trust)
    }

    pub fn allows(&self, facet: &str, kind: Foreign) -> bool {
        match kind {
            Foreign::Native => self.native.contains(facet),
            Foreign::Bridge => self.bridge.contains(facet),
        }
    }
}

impl Trust {
    /// Why a facet the application does not trust is refused, and how to
    /// trust it: its line of the `Facetfile` as it should read (its source
    /// and version kept), or the line to add if it has none.
    pub fn refusal(&self, name: &str, kind: Foreign) -> String {
        let option = kind.option();
        let how = match self.written.get(name) {
            Some(written) => {
                let off = format!("{option}: false");
                let trusted = if written.contains(&off) {
                    written.replacen(&off, &format!("{option}: true"), 1)
                } else {
                    format!("{written}, {option}: true")
                };
                format!("add `{option}: true` to its line in the Facetfile: `{trusted}`")
            }
            None => format!("trust it with `facet \"{name}\", {option}: true` in the Facetfile"),
        };
        format!("facet `{name}` ships {}, which runs outside Grenat's sandbox: {how}", kind.what())
    }
}

/// Refuses the facets among `facets` (name, directory) whose code outside
/// Grenat the package in `root` does not trust.
pub fn check(root: &Path, facets: &[(String, PathBuf)]) -> Result<(), String> {
    let trust = Trust::load(root)?;
    for (name, dir) in facets {
        if let Some(kind) = Foreign::of(&Manifest::load(dir)?)
            && !trust.allows(name, kind)
        {
            return Err(trust.refusal(name, kind));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_code_is_trusted_by_its_own_option() {
        let dir = std::env::temp_dir().join(format!("grenat-trust-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Facetfile"), "facet \"a\", native: true\nfacet \"b\", bridge: true\nfacet \"c\"\n")
            .unwrap();
        let trust = Trust::load(&dir).unwrap();
        assert!(trust.allows("a", Foreign::Native) && !trust.allows("a", Foreign::Bridge));
        assert!(trust.allows("b", Foreign::Bridge) && !trust.allows("b", Foreign::Native));
        assert!(!trust.allows("c", Foreign::Native) && !trust.allows("c", Foreign::Bridge));
        assert_eq!(Trust::load(Path::new("/nowhere")).unwrap(), Trust::default());
        // not in the Facetfile (required by another facet): the line to add
        assert_eq!(
            Trust::default().refusal("texts", Foreign::Bridge),
            "facet `texts` ships a bridge (Ruby or Python code, run as a process), which runs outside Grenat's \
             sandbox: trust it with `facet \"texts\", bridge: true` in the Facetfile"
        );
        // in it: its own line, its source and version kept
        std::fs::write(
            dir.join("Facetfile"),
            "facet \"texts\", path: \"../texts\"\nfacet \"sheets\", \"~> 0.1\", git: \"https://x.io/s\", native: false\n",
        )
        .unwrap();
        let trust = Trust::load(&dir).unwrap();
        assert_eq!(
            trust.refusal("texts", Foreign::Bridge),
            "facet `texts` ships a bridge (Ruby or Python code, run as a process), which runs outside Grenat's \
             sandbox: add `bridge: true` to its line in the Facetfile: `facet \"texts\", path: \"../texts\", bridge: true`"
        );
        assert!(
            trust.refusal("sheets", Foreign::Native).ends_with(
                "add `native: true` to its line in the Facetfile: `facet \"sheets\", \"~> 0.1\", git: \"https://x.io/s\", \
                 native: true`"
            ),
            "{}",
            trust.refusal("sheets", Foreign::Native)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
