//! The quotes of a CSV text, checked before it is parsed.
//!
//! The `csv` crate is lenient where a text is malformed: a quote that is
//! never closed takes every line after it into one cell, and a quoted cell
//! followed by more text (`"a"b`) loses its quotes (`ab`). Either would
//! give rows that are not the text's, without a word, so both are errors
//! here, each naming the line where it is:
//!
//! - a quote opens a cell only at its start (`a,"b"`), and closes it when
//!   it is not doubled (`""` is a quote inside the cell);
//! - after the closing quote come a delimiter or the end of the line;
//! - a quote elsewhere (`5" disk`) is a character of its cell, as the
//!   `csv` crate reads it.

/// Whether the quoted cells of `text` (delimited by `delimiter`) are
/// closed, and followed by a delimiter or the end of a line.
pub fn check(text: &str, delimiter: u8) -> Result<(), String> {
    let bytes = text.as_bytes();
    let mut line = 1;
    // the line of the quote that opened the cell being read, if quoted
    let mut opened: Option<usize> = None;
    let mut cell_start = true;
    let mut closed = false;
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'\n' {
            line += 1;
        }
        if opened.is_some() {
            if byte == b'"' {
                if bytes.get(i + 1) == Some(&b'"') {
                    i += 1;
                } else {
                    (opened, closed) = (None, true);
                }
            }
        } else if byte == delimiter || byte == b'\n' || byte == b'\r' {
            (cell_start, closed) = (true, false);
        } else if closed {
            let after = text[i..].chars().next().unwrap_or_default();
            return Err(format!(
                "line {line}: a quoted cell is followed by {after:?}, where a delimiter or the end of the line must be"
            ));
        } else if cell_start && byte == b'"' {
            (opened, cell_start) = (Some(line), false);
        } else {
            cell_start = false;
        }
        i += 1;
    }
    match opened {
        Some(line) => Err(format!("line {line}: a quoted cell is never closed")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_quoted_texts_pass() {
        for text in [
            "",
            "a,b\n1,2\n",
            "\"a, b\",\"said \"\"hi\"\"\"\r\n",
            "x,\"two\nlines\"\n\"\",\"\"",
            "5\" disk,a\"b\n",
            " \"a\" ,b\n",
            "\"a\"\r\n\"b\"\r",
            "\"é\",\"ü\"",
        ] {
            assert_eq!(check(text, b','), Ok(()), "{text:?}");
        }
        assert_eq!(check("\"a;b\";c\n", b';'), Ok(()));
        assert_eq!(check("\"a\"\t\"b\"\n", b'\t'), Ok(()));
    }

    #[test]
    fn a_quote_never_closed_names_its_line() {
        assert_eq!(check("a,\"unterminated\nb,c\nd,e\n", b','), Err("line 1: a quoted cell is never closed".into()));
        assert_eq!(check("x\ny\n\"two\nlines\n", b','), Err("line 3: a quoted cell is never closed".into()));
        assert_eq!(check("\"a\"\"", b','), Err("line 1: a quoted cell is never closed".into()));
        // a delimiter of its own: the comma is inside the cell
        assert!(check("\"a\";\"b,\n", b';').is_err());
    }

    #[test]
    fn text_after_a_closing_quote_names_its_line() {
        assert_eq!(
            check("x\n\"a\"b,c\n", b','),
            Err("line 2: a quoted cell is followed by 'b', where a delimiter or the end of the line must be".into())
        );
        assert_eq!(
            check("\"two\nlines\" é\n", b','),
            Err("line 2: a quoted cell is followed by ' ', where a delimiter or the end of the line must be".into())
        );
        assert!(check("\"a\"\"\"\"b\"\n", b',').is_ok());
        assert!(check("\"a\";b\n", b',').is_err());
    }
}
