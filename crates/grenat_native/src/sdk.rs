//! Which `grenat_ext` a facet's crate is built against.
//!
//! A facet's crate depends on the SDK by version (`grenat_ext = "0.1"`),
//! which cargo fetches from crates.io. Where a Grenat checkout is at hand,
//! the build uses the checkout's own crate instead — offline, before a
//! release is published, and always at the version of this Grenat — by
//! passing cargo a patch of crates.io
//! (`--config 'patch.crates-io.grenat_ext.path="…"'`). The checkout is, in
//! order:
//!
//! 1. the directory [`SDK_ENV`] names (`GRENAT_EXT_PATH`): a Grenat checkout
//!    or the `grenat_ext` crate itself; set but empty, crates.io (no patch);
//! 2. the Grenat checkout the facet's crate lies in (`facets/html/native`);
//! 3. the checkout this Grenat was built from, if it is still there.
//!
//! Elsewhere (a Grenat installed from a release, a facet fetched from an
//! index or a git repository), nothing is patched: cargo fetches the
//! published crate.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The environment variable naming where `grenat_ext` lives.
pub const SDK_ENV: &str = "GRENAT_EXT_PATH";

/// The SDK's crate, in a Grenat checkout.
const IN_CHECKOUT: &str = "crates/grenat_ext";

/// The arguments that make cargo build the crate in `crate_dir` against
/// the local `grenat_ext`, if there is one (none otherwise, nor for a crate
/// that does not use the SDK).
pub fn cargo_args(crate_dir: &Path) -> Result<Vec<OsString>, String> {
    let manifest = std::fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap_or_default();
    if !manifest.contains("grenat_ext") {
        return Ok(Vec::new());
    }
    let explicit = std::env::var_os(SDK_ENV);
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Ok(match locate(explicit.as_deref().map(Path::new), crate_dir, &built_from)? {
        Some(sdk) => patch(&sdk),
        None => Vec::new(),
    })
}

/// The `grenat_ext` crate to build against: `explicit` (the variable's
/// value), else a checkout around `crate_dir`, else `built_from`'s.
pub fn locate(explicit: Option<&Path>, crate_dir: &Path, built_from: &Path) -> Result<Option<PathBuf>, String> {
    if let Some(dir) = explicit {
        if dir.as_os_str().is_empty() {
            return Ok(None);
        }
        return sdk_in(dir)
            .map(Some)
            .ok_or_else(|| format!("{SDK_ENV}={}: neither a Grenat checkout nor the grenat_ext crate", dir.display()));
    }
    let crate_dir = crate_dir.canonicalize().unwrap_or_else(|_| crate_dir.to_path_buf());
    if let Some(sdk) =
        crate_dir.ancestors().find_map(|dir| is_sdk(&dir.join(IN_CHECKOUT)).then(|| dir.join(IN_CHECKOUT)))
    {
        return Ok(Some(sdk));
    }
    Ok(sdk_in(built_from))
}

/// The `grenat_ext` crate `dir` is, or holds as a Grenat checkout.
fn sdk_in(dir: &Path) -> Option<PathBuf> {
    [dir.to_path_buf(), dir.join(IN_CHECKOUT)]
        .into_iter()
        .find(|candidate| is_sdk(candidate))
        .map(|sdk| sdk.canonicalize().unwrap_or(sdk))
}

/// Whether `dir` is the `grenat_ext` crate.
fn is_sdk(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Cargo.toml"))
        .is_ok_and(|text| text.lines().any(|line| line.trim() == "name = \"grenat_ext\""))
}

/// `--config 'patch.crates-io.grenat_ext.path="<sdk>"'`.
fn patch(sdk: &Path) -> Vec<OsString> {
    // a TOML string: JSON's escapes are TOML's
    let path = serde_json::to_string(&sdk.to_string_lossy()).expect("a path");
    vec!["--config".into(), format!("patch.crates-io.grenat_ext.path={path}").into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of the system's temporary one, removed at the end.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("grenat-native-sdk-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir.canonicalize().unwrap())
        }

        fn write(&self, path: &str, text: &str) -> PathBuf {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path.parent().unwrap().to_path_buf()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SDK: &str = "[package]\nname = \"grenat_ext\"\nversion = \"0.1.5\"\n";
    const FACET: &str = "[package]\nname = \"html_native\"\n\n[dependencies]\ngrenat_ext = \"0.1\"\n";

    #[test]
    fn a_crate_inside_a_checkout_is_built_against_it() {
        let dir = Scratch::new("inside");
        let sdk = dir.write("grenat/crates/grenat_ext/Cargo.toml", SDK);
        let facet = dir.write("grenat/facets/html/native/Cargo.toml", FACET);
        let elsewhere = dir.0.join("nowhere");
        assert_eq!(locate(None, &facet, &elsewhere).unwrap(), Some(sdk));
    }

    #[test]
    fn a_crate_outside_falls_back_on_the_checkout_grenat_was_built_from() {
        let dir = Scratch::new("outside");
        let sdk = dir.write("grenat/crates/grenat_ext/Cargo.toml", SDK);
        let facet = dir.write("app/.grenat/facets/html-0.1.0/native/Cargo.toml", FACET);
        assert_eq!(locate(None, &facet, &dir.0.join("grenat")).unwrap(), Some(sdk));
        // a Grenat installed from a release: crates.io
        assert_eq!(locate(None, &facet, &dir.0.join("gone")).unwrap(), None);
    }

    #[test]
    fn the_variable_names_a_checkout_or_the_crate_or_crates_io() {
        let dir = Scratch::new("variable");
        let sdk = dir.write("sdk/crates/grenat_ext/Cargo.toml", SDK);
        let other = dir.write("grenat/crates/grenat_ext/Cargo.toml", SDK);
        let facet = dir.write("grenat/facets/html/native/Cargo.toml", FACET);
        let built_from = dir.0.join("grenat");
        assert_eq!(locate(Some(&dir.0.join("sdk")), &facet, &built_from).unwrap(), Some(sdk.clone()));
        assert_eq!(locate(Some(&sdk), &facet, &built_from).unwrap(), Some(sdk));
        assert_eq!(locate(Some(Path::new("")), &facet, &built_from).unwrap(), None);
        assert_eq!(locate(None, &facet, &built_from).unwrap(), Some(other));
        let e = locate(Some(&dir.0.join("missing")), &facet, &built_from).unwrap_err();
        assert!(e.starts_with("GRENAT_EXT_PATH=") && e.ends_with("neither a Grenat checkout nor the grenat_ext crate"));
    }

    #[test]
    fn another_crate_named_alike_is_no_sdk() {
        let dir = Scratch::new("alike");
        dir.write("grenat/crates/grenat_ext/Cargo.toml", "[package]\nname = \"grenat_ext_macros\"\n");
        let facet = dir.write("grenat/facets/html/native/Cargo.toml", FACET);
        assert_eq!(locate(None, &facet, &dir.0.join("gone")).unwrap(), None);
    }

    #[test]
    fn the_patch_is_a_cargo_config_value() {
        let args = patch(Path::new("/src/grenat \"x\"/crates/grenat_ext"));
        assert_eq!(args[0], "--config");
        assert_eq!(args[1], "patch.crates-io.grenat_ext.path=\"/src/grenat \\\"x\\\"/crates/grenat_ext\"");
    }

    #[test]
    fn a_crate_without_the_sdk_is_not_patched() {
        let dir = Scratch::new("plain");
        let plain = dir.write("old_abi/Cargo.toml", "[package]\nname = \"old_abi\"\n");
        assert!(cargo_args(&plain).unwrap().is_empty());
    }
}
