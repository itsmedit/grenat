//! Byte offsets ↔ LSP positions (line, UTF-16 column).

use serde_json::{Value as Json, json};

pub(crate) struct LineIndex<'t> {
    text: &'t str,
    /// Byte offset of each line's start.
    starts: Vec<usize>,
}

impl<'t> LineIndex<'t> {
    pub(crate) fn new(text: &'t str) -> LineIndex<'t> {
        let starts = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
        LineIndex { text, starts }
    }

    pub(crate) fn position(&self, offset: usize) -> Json {
        let offset = offset.min(self.text.len());
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let column: usize = self.text[self.starts[line]..offset].chars().map(char::len_utf16).sum();
        json!({"line": line, "character": column})
    }

    pub(crate) fn range(&self, start: usize, end: usize) -> Json {
        json!({"start": self.position(start), "end": self.position(end.max(start))})
    }

    /// The byte offset of an LSP position (clamped to the text).
    pub(crate) fn offset(&self, position: &Json) -> usize {
        let line = position["line"].as_u64().unwrap_or(0) as usize;
        let Some(&start) = self.starts.get(line) else { return self.text.len() };
        let end = self.starts.get(line + 1).map_or(self.text.len(), |&e| e);
        let mut units = position["character"].as_u64().unwrap_or(0) as usize;
        for (i, c) in self.text[start..end].char_indices() {
            if units == 0 || c == '\n' {
                return start + i;
            }
            units = units.saturating_sub(c.len_utf16());
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_utf16_units() {
        let index = LineIndex::new("ab\n\"é😀x\"\n");
        assert_eq!(index.position(0), json!({"line": 0, "character": 0}));
        let x = "ab\n\"é😀".len();
        assert_eq!(index.position(x), json!({"line": 1, "character": 4}));
        assert_eq!(index.offset(&json!({"line": 1, "character": 4})), x);
        assert_eq!(index.offset(&json!({"line": 0, "character": 99})), 2);
        assert_eq!(index.offset(&json!({"line": 9, "character": 0})), index.text.len());
    }
}
