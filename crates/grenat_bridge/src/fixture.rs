//! Facets with a bridge, for tests (the `fixture` feature): `texts`
//! (`fixtures/texts`, served in Ruby) and `numbers` (`fixtures/numbers`,
//! served in Python), each installed once per test binary.
//!
//! No Ruby or Python is installed by the tests: where the interpreter is
//! missing, [`installed`] says why and gives nothing, and the test that
//! wanted it returns early.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::install::{Installed, install};
use crate::spec::Spec;

/// The facet served in Ruby.
pub const RUBY: &str = "texts";
/// The facet served in Python.
pub const PYTHON: &str = "numbers";

/// The fixture's `[bridge]`, as its `grenat.toml` says.
pub fn spec(facet: &str) -> Spec {
    match facet {
        RUBY => Spec { env: vec!["TEXTS_GREETING".into()], ..Spec::new(&["ruby", "bridge/server.rb"]) },
        PYTHON => Spec::new(&["python3", "bridge/server.py"]),
        other => panic!("no fixture facet `{other}`"),
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Why the interpreter of `facet` cannot serve it here, if it cannot.
pub fn missing(facet: &str) -> Option<String> {
    let (program, check) = match facet {
        RUBY => ("ruby", ["-e", "require 'json'"]),
        _ => ("python3", ["-c", "import json"]),
    };
    let ran =
        Command::new(program).args(check).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    match ran {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!("`{program}` does not work here ({status})")),
        Err(e) => Some(format!("`{program}` is not installed ({e})")),
    }
}

/// A copy of the facet `facet` in `into` (`into/<facet>`).
pub fn copy_facet(facet: &str, into: &Path) -> PathBuf {
    let to = into.join(facet);
    copy_dir(&fixtures().join(facet), &to);
    to
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a directory");
    for entry in std::fs::read_dir(from).expect("the fixture") {
        let entry = entry.expect("an entry");
        let name = entry.file_name();
        if name == ".grenat" || name == "__pycache__" {
            continue;
        }
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to.join(&name));
        } else {
            std::fs::copy(entry.path(), to.join(&name)).expect("a copy");
        }
    }
}

/// The facet `facet` (a copy in a directory of the system's temporary one)
/// and its bridge, installed once; `None`, the reason printed, where its
/// interpreter is missing.
pub fn installed(facet: &str) -> Option<&'static (PathBuf, Installed)> {
    static RUBY_FACET: OnceLock<Option<(PathBuf, Installed)>> = OnceLock::new();
    static PYTHON_FACET: OnceLock<Option<(PathBuf, Installed)>> = OnceLock::new();
    let cell = if facet == RUBY { &RUBY_FACET } else { &PYTHON_FACET };
    let found = cell.get_or_init(|| {
        if missing(facet).is_some() {
            return None;
        }
        let dir = std::env::temp_dir().join(format!("grenat-bridge-fixture-{}-{facet}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let copy = copy_facet(facet, &dir);
        let installed = install(facet, &copy, &spec(facet))
            .unwrap_or_else(|e| panic!("the fixture `{facet}` cannot be installed: {e}"));
        Some((copy, installed))
    });
    if found.is_none() {
        println!("skipped: the bridge facet `{facet}` cannot run here: {}", missing(facet).unwrap_or_default());
    }
    found.as_ref()
}

/// The Grenat declarations of the installed facet `facet`.
pub fn declarations(facet: &str) -> Option<String> {
    let (dir, _) = installed(facet)?;
    Some(std::fs::read_to_string(crate::layout::Layout::new(dir).declarations()).expect("declarations"))
}
