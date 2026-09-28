//! Where a facet's native part lives once installed:
//!
//! ```text
//! <facet>/.grenat/native/
//!   <arch>-<os>/lib<facet>.<dylib|so|dll>   the library, for this platform
//!   manifest.json                           what it exports (read from it)
//!   native.grn                              the Grenat declarations of that
//! ```
//!
//! A facet may also ship its libraries already built, one per platform:
//! `<crate>/prebuilt/<arch>-<os>/lib<facet>.<ext>`, used instead of
//! building when present.

use std::path::{Path, PathBuf};

/// The directory of the native part, in an installed facet.
pub const NATIVE_DIR: &str = ".grenat/native";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const DECLARATIONS_FILE: &str = "native.grn";

/// This platform: `aarch64-macos`, `x86_64-linux`.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// The file name of the library of `facet` on this platform: `libsheets.so`.
pub fn library_file(facet: &str) -> String {
    format!("{}{facet}{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX)
}

/// The native part of the facet `facet`, installed in `dir`.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub facet: String,
    pub dir: PathBuf,
}

impl Layout {
    pub fn new(facet: &str, facet_dir: &Path) -> Layout {
        Layout { facet: facet.to_string(), dir: facet_dir.join(NATIVE_DIR) }
    }

    pub fn library(&self) -> PathBuf {
        self.dir.join(platform()).join(library_file(&self.facet))
    }

    pub fn manifest(&self) -> PathBuf {
        self.dir.join(MANIFEST_FILE)
    }

    pub fn declarations(&self) -> PathBuf {
        self.dir.join(DECLARATIONS_FILE)
    }
}

/// The library the facet ships for this platform, in its crate `crate_dir`.
pub fn prebuilt(facet: &str, crate_dir: &Path) -> PathBuf {
    crate_dir.join("prebuilt").join(platform()).join(library_file(facet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_depend_on_the_platform() {
        let layout = Layout::new("sheets", Path::new("/app/.grenat/facets/sheets-0.1.0"));
        let library = layout.library();
        assert!(library.starts_with("/app/.grenat/facets/sheets-0.1.0/.grenat/native"));
        assert_eq!(library.parent().unwrap().file_name().unwrap().to_str().unwrap(), platform());
        let file = library.file_name().unwrap().to_str().unwrap().to_string();
        assert!(file.contains("sheets"), "{file}");
        if cfg!(target_os = "linux") {
            assert_eq!(file, "libsheets.so");
        } else if cfg!(target_os = "macos") {
            assert_eq!(file, "libsheets.dylib");
        }
        assert!(layout.manifest().ends_with(".grenat/native/manifest.json"));
        assert!(layout.declarations().ends_with(".grenat/native/native.grn"));
        assert_eq!(
            prebuilt("sheets", Path::new("/f/native")),
            Path::new("/f/native/prebuilt").join(platform()).join(file)
        );
    }
}
