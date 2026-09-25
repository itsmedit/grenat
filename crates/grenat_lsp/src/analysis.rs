//! Checking the program of a document: its diagnostics, and the parsed
//! program for navigation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use grenat_ast::{Diagnostic, Program, Span};
use grenat_package::LoadError;
use grenat_report::{SourceFile, Sources};
use serde_json::{Value as Json, json};

use crate::position::LineIndex;

pub(crate) struct Analysis {
    pub sources: Sources,
    /// `None` when the program does not parse.
    pub program: Option<Program>,
    /// Spans in `sources`.
    pub diagnostics: Vec<Diagnostic>,
}

impl Analysis {
    /// The program whose entry is `path` (the open document), `overlay`
    /// holding the open documents' texts.
    pub(crate) fn of(path: &Path, overlay: &HashMap<PathBuf, String>) -> Analysis {
        let bundle = match grenat_package::load_with(path, false, overlay) {
            Ok(bundle) => bundle,
            Err(LoadError::Diagnostics { sources, diagnostics }) => {
                return Analysis { sources, program: None, diagnostics };
            }
            Err(LoadError::Message(message)) => {
                let text = overlay.get(path).cloned().unwrap_or_default();
                let sources = Sources::single(&path.to_string_lossy(), &text);
                return Analysis { sources, program: None, diagnostics: vec![Diagnostic::new(Span::default(), message)] };
            }
        };
        let mut parsed = grenat_parser::parse(&bundle.sources.text);
        grenat_package::strip_requires(&mut parsed.program);
        let mut diagnostics = parsed.diagnostics;
        if diagnostics.is_empty() {
            diagnostics = grenat_macros::expand(&mut parsed.program, &bundle.sources.text);
        }
        if diagnostics.is_empty() {
            diagnostics = grenat_types::check(&parsed.program);
        }
        Analysis { sources: bundle.sources, program: Some(parsed.program), diagnostics }
    }

    /// The file of the program that is `path`.
    pub(crate) fn file(&self, path: &Path) -> Option<&SourceFile> {
        self.sources.files.iter().find(|f| same_file(&f.path, path))
    }

    /// The diagnostics in `path`, as LSP diagnostics.
    pub(crate) fn lsp_diagnostics(&self, path: &Path) -> Vec<Json> {
        let Some(file) = self.file(path) else { return Vec::new() };
        let index = LineIndex::new(self.sources.text_of(file));
        self.diagnostics
            .iter()
            .filter_map(|diag| {
                let (in_file, span) = self.sources.locate(diag.span);
                if in_file != file {
                    return None;
                }
                let mut message = diag.message.clone();
                if let Some(help) = &diag.help {
                    message.push_str(&format!("\nhelp: {help}"));
                }
                let mut lsp = json!({
                    "range": index.range(span.start as usize, span.end as usize),
                    "severity": 1,
                    "source": "grenat",
                    "message": message,
                });
                if let Some(code) = diag.code {
                    lsp["code"] = json!(code);
                }
                Some(lsp)
            })
            .collect()
    }
}

/// Whether `shown` (a path as the loader shows it) is the file `path`.
pub(crate) fn same_file(shown: &str, path: &Path) -> bool {
    let shown = Path::new(shown);
    shown == path || shown.canonicalize().is_ok_and(|c| c == path)
}
