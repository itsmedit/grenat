//! The header fields a MIME parser could read in a message's bytes, found
//! without parsing it: every `Content-Type` and `Content-Transfer-Encoding`
//! field, wherever one could start, and the header block each belongs to.
//!
//! The search errs on the side of finding too much, never too little, as
//! `mail-parser` reads headers: a field name ignores case and any white
//! space inside it, and may start mid-line (right after a MIME boundary),
//! so every `:` whose preceding text, white space removed, ends with the
//! name may start one; a value runs to the end of its line and over the
//! lines that start with a space or a tab; a header block ends at a line
//! of white space only that does not start with a space or a tab.
//!
//! In linear time, whatever the bytes: one field per name and line, its
//! value taken from the line's first possible start (which holds every
//! later one); a folded line belongs to the field above it unless it may
//! start one of its own.

/// The fields that give a message its structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Name {
    ContentType,
    TransferEncoding,
}

/// A field that may start on a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Field<'a> {
    pub name: Name,
    /// Its header block's index (blocks are separated by empty lines).
    pub block: usize,
    /// The value from each place on the line where the field may start
    /// (to the end of the line), first to last.
    pub starts: Vec<&'a [u8]>,
    /// The value's lines: the first start's, then the lines folded into it.
    pub lines: Vec<&'a [u8]>,
}

const CONTENT_TYPE: &[u8] = b"content-type";
const TRANSFER_ENCODING: &[u8] = b"content-transfer-encoding";

/// Every `Content-Type` and `Content-Transfer-Encoding` field `raw` may hold.
pub(crate) fn fields(raw: &[u8]) -> Vec<Field<'_>> {
    let mut found: Vec<Field> = Vec::new();
    // the field each name's folded lines go to, as an index in `found`
    let mut open: [Option<usize>; 2] = [None, None];
    let mut block = 0;
    for line in raw.split(|b| *b == b'\n') {
        if ends_a_block(line) {
            block += 1;
            open = [None, None];
            continue;
        }
        let folded = matches!(line.first(), Some(b' ' | b'\t'));
        for (slot, name, field_name) in
            [(0, Name::ContentType, CONTENT_TYPE), (1, Name::TransferEncoding, TRANSFER_ENCODING)]
        {
            let starts: Vec<&[u8]> =
                colons(line).filter(|&c| named(&line[..c], field_name)).map(|c| &line[c + 1..]).collect();
            if let Some(&first) = starts.first() {
                open[slot] = Some(found.len());
                found.push(Field { name, block, starts, lines: vec![first] });
            } else if let Some(at) = open[slot].filter(|_| folded) {
                found[at].lines.push(line);
            } else {
                open[slot] = None;
            }
        }
    }
    found
}

fn colons(line: &[u8]) -> impl Iterator<Item = usize> + '_ {
    line.iter().enumerate().filter(|(_, b)| **b == b':').map(|(at, _)| at)
}

/// An empty line (`\r` or white space alone, not folded) ends a header block.
fn ends_a_block(line: &[u8]) -> bool {
    !matches!(line.first(), Some(b' ' | b'\t')) && line.iter().all(u8::is_ascii_whitespace)
}

/// Whether `before`, its white space removed, ends with `name` (lowercase), ignoring case.
fn named(before: &[u8], name: &[u8]) -> bool {
    let mut letters = before.iter().rev().filter(|b| !b.is_ascii_whitespace());
    name.iter().rev().all(|expected| letters.next().is_some_and(|b| b.eq_ignore_ascii_case(expected)))
}

/// Whether `haystack` holds `needle` (lowercase), ignoring case.
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Each field: its name, its value's lines, its block.
    fn found(raw: &str) -> Vec<(Name, Vec<String>, usize)> {
        fields(raw.as_bytes()).into_iter().map(|f| (f.name, f.lines.into_iter().map(text).collect(), f.block)).collect()
    }

    #[test]
    fn fields_and_their_blocks() {
        let raw = "From: a@b.c\r\nContent-Type: multipart/mixed;\r\n\tboundary=x\r\n\r\n--x\r\nCONTENT-transfer-ENCODING: base64\r\n\r\nQUJD\r\n";
        assert_eq!(
            found(raw),
            [
                (Name::ContentType, vec![" multipart/mixed;\r".into(), "\tboundary=x\r".into()], 0),
                (Name::TransferEncoding, vec![" base64\r".into()], 1),
            ]
        );
    }

    #[test]
    fn names_as_a_lenient_parser_reads_them() {
        // white space inside the name, a field right after a boundary, a value folded
        let raw = "  Con tent-Ty pe : message/rfc822\n--bContent-Type:\n message/global\nx: y: Content-Type: a/b\n";
        let values: Vec<Vec<String>> = found(raw).into_iter().map(|(_, v, _)| v).collect();
        assert_eq!(values, [vec![" message/rfc822"], vec!["", " message/global"], vec![" a/b"]]);
        // not the name
        assert!(found("X-Content-Typer: a\nContent-Types: b\n").is_empty());
        // every place a field may start on a line, the first holding the others
        let field = &fields(b"content-type: a content-type: b/c")[0];
        assert_eq!(field.starts.iter().map(|s| text(s)).collect::<Vec<_>>(), [" a content-type: b/c", " b/c"]);
        assert_eq!(field.lines, [field.starts[0]]);
    }

    #[test]
    fn blocks_end_at_empty_lines_only() {
        // a line of spaces folds into the value above it; `\r` alone ends the block
        let raw = "Content-Type: a\n \nContent-Transfer-Encoding: b\n\r\nContent-Type: c\n";
        let blocks: Vec<usize> = found(raw).into_iter().map(|(_, _, b)| b).collect();
        assert_eq!(blocks, [0, 0, 1]);
    }

    #[test]
    fn a_folded_line_that_may_start_a_field_starts_its_own() {
        // linear time: no line belongs to two fields of one name
        let raw = "Content-Type: a\n content-type: b\n c\n".to_string() + &" content-type: x\n".repeat(1000);
        let fields = fields(raw.as_bytes());
        assert_eq!(fields.len(), 1002);
        assert_eq!(fields[1].lines.iter().map(|l| text(l)).collect::<Vec<_>>(), [" b", " c"]);
        assert_eq!(fields.iter().map(|f| f.lines.len()).sum::<usize>(), 1003);
    }
}
