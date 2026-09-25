//! The files of a program, one after the other in a single text.

use grenat_ast::Span;

/// One file of a program: where its text starts in [`Sources::text`].
#[derive(Debug, Clone, PartialEq)]
pub struct SourceFile {
    pub path: String,
    pub base: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sources {
    /// Every file's text, in order.
    pub text: String,
    /// Sorted by `base`; never empty.
    pub files: Vec<SourceFile>,
}

impl Sources {
    pub fn single(path: &str, text: &str) -> Sources {
        Sources { text: text.to_string(), files: vec![SourceFile { path: path.to_string(), base: 0 }] }
    }

    /// `files` as (path, text), in order.
    pub fn join<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Sources {
        let mut sources = Sources { text: String::new(), files: Vec::new() };
        for (path, text) in files {
            sources.push(path, text);
        }
        sources
    }

    /// Appends a file: separated from the previous one by a blank line, so
    /// that nothing (a `##` comment) runs from one file into the next.
    pub fn push(&mut self, path: &str, text: &str) {
        if !self.text.is_empty() {
            if !self.text.ends_with('\n') {
                self.text.push('\n');
            }
            self.text.push('\n');
        }
        self.files.push(SourceFile { path: path.to_string(), base: self.text.len() as u32 });
        self.text.push_str(text);
    }

    /// The file `span` points into, and `span` within that file.
    pub fn locate(&self, span: Span) -> (&SourceFile, Span) {
        let i = self.files.partition_point(|f| f.base <= span.start).saturating_sub(1);
        let file = &self.files[i];
        (file, Span { start: span.start - file.base, end: span.end.saturating_sub(file.base) })
    }

    /// The text of one file.
    pub fn text_of(&self, file: &SourceFile) -> &str {
        let next = self.files.iter().find(|f| f.base > file.base).map_or(self.text.len(), |f| f.base as usize);
        let text = &self.text[file.base as usize..next];
        // without the blank line separating it from the next file
        if next < self.text.len() { text.strip_suffix('\n').unwrap_or(text) } else { text }
    }

    /// The file table, as text: one `base path` line per file (the embedded
    /// form of built executables).
    pub fn table(&self) -> String {
        self.files.iter().map(|f| format!("{} {}\n", f.base, f.path)).collect()
    }

    /// Sources from `text` and its [`table`](Self::table); a table that is
    /// a bare path (or invalid) makes a single file.
    pub fn from_table(text: &str, table: &str) -> Sources {
        let files: Option<Vec<SourceFile>> = table
            .lines()
            .map(|line| {
                let (base, path) = line.split_once(' ')?;
                Some(SourceFile { path: path.to_string(), base: base.parse().ok()? })
            })
            .collect();
        match files {
            Some(files) if files.first().is_some_and(|f| f.base == 0) => Sources { text: text.to_string(), files },
            _ => Sources::single(table.trim_end(), text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_are_located_in_their_file() {
        let sources = Sources::join([("a.grn", "x = 1\n"), ("lib/b.grn", "def f\nend")]);
        assert_eq!(sources.text, "x = 1\n\ndef f\nend");
        let (file, span) = sources.locate(Span { start: 11, end: 14 });
        assert_eq!((file.path.as_str(), span), ("lib/b.grn", Span { start: 4, end: 7 }));
        assert_eq!(sources.text_of(file), "def f\nend");
        assert_eq!(sources.text_of(&sources.files[0]), "x = 1\n");
        assert_eq!(sources.locate(Span { start: 2, end: 3 }).0.path, "a.grn");
    }

    #[test]
    fn the_table_round_trips() {
        let sources = Sources::join([("a.grn", "x\n"), ("b c.grn", "y\n")]);
        assert_eq!(sources.table(), "0 a.grn\n3 b c.grn\n");
        assert_eq!(Sources::from_table(&sources.text, &sources.table()), sources);
        assert_eq!(Sources::from_table("x\n", "prog"), Sources::single("prog", "x\n"));
    }

    #[test]
    fn a_diagnostic_shows_its_own_file() {
        let sources = Sources::join([("main.grn", "helper\n"), ("lib.grn", "def helper = 1 +\n")]);
        let mut diag = grenat_ast::Diagnostic::new(Span { start: 23, end: 24 }, "expected an expression");
        diag = diag.with_note(Span { start: 0, end: 6 }, "called here");
        let text = crate::render_in(&sources, &diag, false);
        assert!(text.contains("--> lib.grn:1:16\n"), "{text}");
        assert!(text.contains("1 | def helper = 1 +\n"), "{text}");
        assert!(text.contains("--> main.grn:1:1\n"), "{text}");
    }
}
