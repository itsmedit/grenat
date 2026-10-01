//! Reading CSV: a text (pure) or a file (`fs.read`, untrusted), as rows of
//! cells or as records (the first row naming the columns).
//!
//! Quoted cells may hold delimiters, quotes (doubled: `""`) and line
//! breaks; rows may differ in length; blank lines are skipped; a UTF-8
//! byte order mark (as Excel writes one) is dropped. A quoted cell never
//! closed, or followed by more than a delimiter or the end of its line, is
//! an error ([`crate::quotes`]).

use grenat_ext::export;

use crate::delimiter::delimiter;
use crate::quotes;
use crate::records::{Record, records};

/// The rows of a CSV text, each a list of its cells.
#[export(pure, error = "CsvError")]
pub fn sheets_parse_csv(text: String, delimiter: String) -> Result<Vec<Vec<String>>, String> {
    parse(&text, &delimiter)
}

/// The records of a CSV text whose first row names the columns.
#[export(pure, error = "CsvError")]
pub fn sheets_parse_csv_records(text: String, delimiter: String) -> Result<Vec<Record>, String> {
    records(parse(&text, &delimiter)?)
}

/// The rows of a CSV file, each a list of its cells.
#[export(effects = "fs.read", error = "CsvError")]
pub fn sheets_read_csv(path: String, delimiter: String) -> Result<Vec<Vec<String>>, String> {
    parse(&read(&path)?, &delimiter).map_err(|e| format!("{path}: {e}"))
}

/// The records of a CSV file whose first row names the columns.
#[export(effects = "fs.read", error = "CsvError")]
pub fn sheets_read_csv_records(path: String, delimiter: String) -> Result<Vec<Record>, String> {
    let rows = parse(&read(&path)?, &delimiter).map_err(|e| format!("{path}: {e}"))?;
    records(rows).map_err(|e| format!("{path}: {e}"))
}

/// The rows of `text`, split on `delimiter`.
pub fn parse(text: &str, delimiter: &str) -> Result<Vec<Vec<String>>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delimiter = self::delimiter(delimiter)?;
    quotes::check(text, delimiter)?;
    let mut reader =
        csv::ReaderBuilder::new().delimiter(delimiter).has_headers(false).flexible(true).from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        rows.push(record.iter().map(str::to_string).collect());
    }
    Ok(rows)
}

/// The text of the file at `path`.
fn read(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("{path} is not UTF-8 text (byte {})", e.utf8_error().valid_up_to()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch;

    fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|row| row.iter().map(|c| c.to_string()).collect()).collect()
    }

    #[test]
    fn quoted_cells_keep_delimiters_quotes_and_lines() {
        let text = "name,note\r\n\"Doe, Jane\",\"said \"\"hi\"\"\"\n\nx,\"two\nlines\"\n,\n";
        assert_eq!(
            sheets_parse_csv(text.into(), ",".into()).unwrap(),
            rows(&[&["name", "note"], &["Doe, Jane", "said \"hi\""], &["x", "two\nlines"], &["", ""]])
        );
    }

    #[test]
    fn delimiters_lengths_and_marks() {
        assert_eq!(
            sheets_parse_csv("a;b;c\n1;2\n".into(), ";".into()).unwrap(),
            rows(&[&["a", "b", "c"], &["1", "2"]])
        );
        assert_eq!(sheets_parse_csv("a\tb\n".into(), "\t".into()).unwrap(), rows(&[&["a", "b"]]));
        assert_eq!(sheets_parse_csv("\u{feff}a,b\n".into(), ",".into()).unwrap(), rows(&[&["a", "b"]]));
        assert_eq!(sheets_parse_csv(String::new(), ",".into()).unwrap(), Vec::<Vec<String>>::new());
        assert!(sheets_parse_csv("a".into(), "::".into()).unwrap_err().starts_with("a delimiter is one"));
    }

    #[test]
    fn malformed_quotes_are_errors() {
        let dir = scratch::Dir::new();
        let e = sheets_parse_csv("a,\"unterminated\nb,c\nd,e\n".into(), ",".into()).unwrap_err();
        assert_eq!(e, "line 1: a quoted cell is never closed");
        let e = sheets_parse_csv("x\n\"a\"b,c\n".into(), ",".into()).unwrap_err();
        assert_eq!(e, "line 2: a quoted cell is followed by 'b', where a delimiter or the end of the line must be");
        let e = sheets_parse_csv_records("name\n\"Ada\n".into(), ",".into()).unwrap_err();
        assert_eq!(e, "line 2: a quoted cell is never closed");
        let path = dir.text("unterminated.csv");
        std::fs::write(&path, "id;label\n1;\"un\n2;deux\n").unwrap();
        let e = sheets_read_csv_records(path.clone(), ";".into()).unwrap_err();
        assert_eq!(e, format!("{path}: line 2: a quoted cell is never closed"));
        // a quote inside a cell is one of its characters
        assert_eq!(sheets_parse_csv("5\" disk,a\n".into(), ",".into()).unwrap(), rows(&[&["5\" disk", "a"]]));
    }

    #[test]
    fn records_from_text() {
        let got = sheets_parse_csv_records("city,zip\nParis,75001\nLyon\n".into(), ",".into()).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["city"], "Paris");
        assert_eq!(got[0]["zip"], "75001");
        assert_eq!(got[1]["zip"], "");
        let e = sheets_parse_csv_records("a,a\n1,2\n".into(), ",".into()).unwrap_err();
        assert_eq!(e, "the column `a` appears twice in the header row");
    }

    #[test]
    fn files_are_read() {
        let dir = scratch::Dir::new();
        let path = dir.text("read.csv");
        std::fs::write(&path, "id;label\n1;\"un; deux\"\n").unwrap();
        assert_eq!(sheets_read_csv(path.clone(), ";".into()).unwrap(), rows(&[&["id", "label"], &["1", "un; deux"]]));
        let got = sheets_read_csv_records(path.clone(), ";".into()).unwrap();
        assert_eq!(got[0]["label"], "un; deux");
        let e = sheets_read_csv(path.clone(), "".into()).unwrap_err();
        assert!(e.starts_with(&format!("{path}: a delimiter is one")), "{e}");
    }

    #[test]
    fn unreadable_files_are_errors() {
        let dir = scratch::Dir::new();
        let missing = dir.text("missing.csv");
        assert!(
            sheets_read_csv(missing.clone(), ",".into()).unwrap_err().starts_with(&format!("cannot read {missing}"))
        );
        assert!(sheets_read_csv_records(missing, ",".into()).is_err());
        let binary = dir.text("binary.csv");
        std::fs::write(&binary, b"ok,\xff\xfe\n").unwrap();
        assert_eq!(
            sheets_read_csv(binary.clone(), ",".into()).unwrap_err(),
            format!("{binary} is not UTF-8 text (byte 3)")
        );
        let ragged = dir.text("ragged.csv");
        std::fs::write(&ragged, "a\n1,2\n").unwrap();
        assert_eq!(
            sheets_read_csv_records(ragged.clone(), ",".into()).unwrap_err(),
            format!("{ragged}: row 2 has 2 cells, the header row 1")
        );
    }
}
