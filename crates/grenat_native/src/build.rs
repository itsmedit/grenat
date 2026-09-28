//! Building a facet's crate: `cargo build --release`, and the `cdylib` it
//! produced, found in cargo's own report (`--message-format=json`) rather
//! than guessed from a target directory.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;

/// Builds the crate in `crate_dir` (its `Cargo.toml`); `target_dir`
/// overrides cargo's (`CARGO_TARGET_DIR`, else the crate's `target/`).
/// Returns the library built.
pub fn build(crate_dir: &Path, target_dir: Option<&Path>) -> Result<PathBuf, String> {
    let manifest = crate_dir.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(format!("no Cargo.toml in {}: a native facet's crate is a Rust crate", crate_dir.display()));
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.args(["build", "--release", "--message-format=json-render-diagnostics", "--manifest-path"]).arg(&manifest);
    if let Some(dir) = target_dir {
        command.arg("--target-dir").arg(dir);
    }
    let out = command.output().map_err(|e| format!("cannot run cargo (is Rust installed? https://rustup.rs): {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(format!("cargo cannot build {}:\n{}", manifest.display(), tail.join("\n")));
    }
    library(&String::from_utf8_lossy(&out.stdout), &manifest)
}

/// The `cdylib` among the artifacts cargo reported (one JSON message a line).
fn library(messages: &str, manifest: &Path) -> Result<PathBuf, String> {
    let canonical = manifest.canonicalize().unwrap_or_else(|_| manifest.to_path_buf());
    let mut found = None;
    for message in messages.lines().filter_map(|line| serde_json::from_str::<Json>(line).ok()) {
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let kinds = message["target"]["crate_types"].as_array().cloned().unwrap_or_default();
        if !kinds.iter().any(|k| k == "cdylib") {
            continue;
        }
        let ours = message["manifest_path"].as_str().is_some_and(|p| Path::new(p) == canonical);
        let file = message["filenames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .find(|f| f.ends_with(std::env::consts::DLL_SUFFIX))
            .map(PathBuf::from);
        if let Some(file) = file
            && (ours || found.is_none())
        {
            found = Some(file);
        }
    }
    found
        .ok_or_else(|| format!("{} builds no library: add `[lib] crate-type = [\"cdylib\"]` to it", manifest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_is_found_in_cargos_report() {
        let suffix = std::env::consts::DLL_SUFFIX;
        let manifest = Path::new("/nowhere/native/Cargo.toml");
        let messages = format!(
            "{{\"reason\":\"compiler-artifact\",\"target\":{{\"crate_types\":[\"lib\"]}},\"filenames\":[\"/t/libserde.rlib\"]}}\n\
             not json\n\
             {{\"reason\":\"compiler-artifact\",\"manifest_path\":\"/nowhere/native/Cargo.toml\",\
             \"target\":{{\"crate_types\":[\"cdylib\"]}},\"filenames\":[\"/t/release/libsheets{suffix}\"]}}\n\
             {{\"reason\":\"build-finished\",\"success\":true}}\n"
        );
        assert_eq!(library(&messages, manifest).unwrap(), PathBuf::from(format!("/t/release/libsheets{suffix}")));
        let e = library("{\"reason\":\"build-finished\"}\n", manifest).unwrap_err();
        assert!(e.contains("crate-type = [\\\"cdylib\\\"]") || e.contains("cdylib"), "{e}");
    }

    #[test]
    fn a_directory_without_a_crate_is_refused() {
        let e = build(Path::new("/nowhere/at/all"), None).unwrap_err();
        assert!(e.starts_with("no Cargo.toml in /nowhere/at/all"), "{e}");
    }
}
