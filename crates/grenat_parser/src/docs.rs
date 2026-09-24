//! Commentaires `##` indexés par ligne, rattachés aux déclarations.

use std::collections::HashMap;

use grenat_ast::*;
use grenat_lexer::Comment;

/// Commentaires `##` indexés par ligne, pour les rattacher aux déclarations.
pub(crate) struct DocTable {
    pub(crate) line_starts: Vec<usize>,
    pub(crate) leading: HashMap<usize, String>,
    pub(crate) trailing: HashMap<usize, String>,
}

impl DocTable {
    pub(crate) fn new(src: &str, comments: &[Comment]) -> Self {
        let line_starts = std::iter::once(0).chain(src.match_indices('\n').map(|(i, _)| i + 1)).collect();
        let mut table = DocTable { line_starts, leading: HashMap::new(), trailing: HashMap::new() };
        for comment in comments.iter().filter(|c| c.doc) {
            let line = table.line_of(comment.span);
            let map = if comment.trailing { &mut table.trailing } else { &mut table.leading };
            map.insert(line, comment.text.clone());
        }
        table
    }

    pub(crate) fn line_of(&self, span: Span) -> usize {
        self.line_starts.partition_point(|&start| start <= span.start as usize) - 1
    }

    /// Lignes `##` contiguës juste au-dessus, puis `##` en fin de la même ligne.
    pub(crate) fn doc_for(&self, span: Span) -> Option<String> {
        let line = self.line_of(span);
        let mut lines = Vec::new();
        let mut above = line;
        while above > 0
            && let Some(text) = self.leading.get(&(above - 1))
        {
            lines.push(text.clone());
            above -= 1;
        }
        lines.reverse();
        lines.extend(self.trailing.get(&line).cloned());
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}
