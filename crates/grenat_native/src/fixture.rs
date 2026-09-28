//! A facet with a native library, for tests (the `fixture` feature):
//! `sheets` (`fixtures/sheets`), whose crate exports pure and effectful
//! functions, a struct, errors and a panic; and a library built for another
//! ABI version (`fixtures/old_abi`).
//!
//! Builds are shared by every test binary (one target directory), and done
//! once per binary.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::install::{Installed, install};

/// The test facet's name.
pub const FACET: &str = "sheets";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Where the fixtures are built: one directory for every test binary.
pub fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => Path::new(&dir).join("native-fixtures"),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/native-fixtures"),
    }
}

/// A copy of the facet `sheets` in `into` (`into/sheets`), its crate
/// depending on this checkout's `grenat_ext`.
pub fn copy_facet(into: &Path) -> PathBuf {
    let to = into.join(FACET);
    copy_dir(&fixtures().join(FACET), &to);
    let manifest = to.join("native/Cargo.toml");
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../grenat_ext").canonicalize().expect("grenat_ext");
    let text = std::fs::read_to_string(&manifest).expect("the fixture's Cargo.toml").replace(
        "path = \"../../../../grenat_ext\"",
        &format!("path = {}", serde_json::to_string(&sdk.to_string_lossy()).expect("a path")),
    );
    std::fs::write(&manifest, text).expect("the copy's Cargo.toml");
    to
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a directory");
    for entry in std::fs::read_dir(from).expect("the fixture") {
        let entry = entry.expect("an entry");
        let name = entry.file_name();
        if name == ".grenat" || name == "target" {
            continue;
        }
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to.join(&name));
        } else {
            std::fs::copy(entry.path(), to.join(&name)).expect("a copy");
        }
    }
}

/// The facet `sheets`, installed once (in a directory of the system's
/// temporary one): its library, manifest and declarations.
pub fn installed() -> &'static (PathBuf, Installed) {
    static INSTALLED: OnceLock<(PathBuf, Installed)> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("grenat-native-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let facet = copy_facet(&dir);
        let installed = install(FACET, &facet, Path::new("native"), Some(&target_dir()))
            .unwrap_or_else(|e| panic!("the fixture facet cannot be installed: {e}"));
        (facet, installed)
    })
}

/// The Grenat declarations of the installed facet `sheets`.
pub fn declarations() -> String {
    let (facet, installed) = installed();
    std::fs::read_to_string(crate::layout::Layout::new(&installed.facet, facet).declarations()).expect("declarations")
}

/// A library that speaks ABI version 0, built once.
pub fn old_abi_library() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        crate::build::build(&fixtures().join("old_abi"), Some(&target_dir()))
            .unwrap_or_else(|e| panic!("the old ABI fixture cannot be built: {e}"))
    })
}
