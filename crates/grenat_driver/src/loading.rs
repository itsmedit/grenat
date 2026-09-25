//! Reading, parsing and checking a program, with its diagnostics printed.

use std::path::Path;

use grenat_ast::Program;
use grenat_package::{LoadError, Package};
use grenat_parser::Diagnostic;
use grenat_report::Sources;

use crate::flags::use_color;

/// A program ready to run.
pub struct Loaded {
    pub sources: Sources,
    pub program: Program,
    /// The package of the entry file, if any.
    pub package: Option<Package>,
}

pub fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).map_err(|e| eprintln!("error: cannot read {path}: {e}")).ok()
}

/// Prints the diagnostics; returns `true` if there are none.
pub fn report(sources: &Sources, diagnostics: &[Diagnostic]) -> bool {
    let color = use_color();
    for diag in diagnostics {
        eprint!("{}", grenat_report::render_in(sources, diag, color));
    }
    diagnostics.is_empty()
}

/// Parses the program made of `sources` (without its `require`s, resolved
/// already), expands its macros, then checks it unless `unchecked`; `None`
/// if it is invalid.
pub fn parse(sources: &Sources, unchecked: bool) -> Option<Program> {
    let mut parsed = grenat_parser::parse(&sources.text);
    if !report(sources, &parsed.diagnostics) {
        return None;
    }
    grenat_package::strip_requires(&mut parsed.program);
    if !report(sources, &grenat_macros::expand(&mut parsed.program, &sources.text)) {
        return None;
    }
    if !unchecked && !report(sources, &grenat_types::check(&parsed.program)) {
        return None;
    }
    Some(parsed.program)
}

/// Reads `path` and the files it requires, parses and checks them.
pub fn load(path: &str, unchecked: bool) -> Option<Loaded> {
    load_with(path, unchecked, false)
}

/// As [`load`]; `update` fetches the latest commits of git dependencies.
pub fn load_with(path: &str, unchecked: bool, update: bool) -> Option<Loaded> {
    let bundle = match grenat_package::load(Path::new(path), update) {
        Ok(bundle) => bundle,
        Err(LoadError::Diagnostics { sources, diagnostics }) => {
            report(&sources, &diagnostics);
            return None;
        }
        Err(LoadError::Message(message)) => {
            eprintln!("error: {message}");
            return None;
        }
    };
    let program = parse(&bundle.sources, unchecked)?;
    Some(Loaded { sources: bundle.sources, program, package: bundle.package })
}
