//! Where a facet's bridge lives once installed:
//!
//! ```text
//! <facet>/.grenat/bridge/
//!   manifest.json          what the server exports (its `describe`)
//!   bridge.grn             the Grenat declarations of that
//!   lib/ruby/grenat/bridge.rb, lib/python/grenat_bridge.py
//!                          the helper libraries, on the server's load path
//! ```

use std::path::{Path, PathBuf};

/// The directory of the bridge, in an installed facet.
pub const BRIDGE_DIR: &str = ".grenat/bridge";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const DECLARATIONS_FILE: &str = "bridge.grn";

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub dir: PathBuf,
}

impl Layout {
    /// The bridge of the facet installed in `facet_dir`.
    pub fn new(facet_dir: &Path) -> Layout {
        Layout { dir: facet_dir.join(BRIDGE_DIR) }
    }

    pub fn manifest(&self) -> PathBuf {
        self.dir.join(MANIFEST_FILE)
    }

    pub fn declarations(&self) -> PathBuf {
        self.dir.join(DECLARATIONS_FILE)
    }

    /// The helper libraries' directory.
    pub fn lib(&self) -> PathBuf {
        self.dir.join("lib")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_in_the_facet() {
        let layout = Layout::new(Path::new("/app/.grenat/facets/texts"));
        assert_eq!(layout.manifest(), Path::new("/app/.grenat/facets/texts/.grenat/bridge/manifest.json"));
        assert_eq!(layout.declarations(), Path::new("/app/.grenat/facets/texts/.grenat/bridge/bridge.grn"));
        assert_eq!(layout.lib(), Path::new("/app/.grenat/facets/texts/.grenat/bridge/lib"));
    }
}
