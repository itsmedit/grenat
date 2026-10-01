//! Writing CSV: a text from rows (pure), or a file (`fs.write`).
//!
//! A cell is quoted when it must be — it holds the delimiter, a quote
//! (doubled), a line break, or it is a row's only cell and empty — and
//! each row ends with `\n`. Rows may differ in length.

use grenat_ext::export;

use crate::delimiter::delimiter;

/// The CSV text of the rows.
#[export(pure, error = "CsvError")]
pub fn sheets_format_csv(rows: Vec<Vec<String>>, delimiter: String) -> Result<String, String> {
    format(&rows, &delimiter)
}

/// Writes the rows to the CSV file at `path`, replacing it.
#[export(effects = "fs.write", error = "CsvError")]
pub fn sheets_write_csv(path: String, rows: Vec<Vec<String>>, delimiter: String) -> Result<(), String> {
    let text = format(&rows, &delimiter)?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {path}: {e}"))
}

/// The CSV text of `rows`, its cells separated by `delimiter`.
pub fn format(rows: &[Vec<String>], delimiter: &str) -> Result<String, String> {
    let mut writer =
        csv::WriterBuilder::new().delimiter(self::delimiter(delimiter)?).flexible(true).from_writer(Vec::new());
    for row in rows {
        writer.write_record(row).map_err(|e| e.to_string())?;
    }
    let bytes = writer.into_inner().map_err(|e| e.to_string())?;
    // cells are strings and the delimiter ASCII: the text is UTF-8
    Ok(String::from_utf8(bytes).expect("CSV of UTF-8 cells"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv_read::parse;
    use crate::scratch;

    fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|row| row.iter().map(|c| c.to_string()).collect()).collect()
    }

    #[test]
    fn cells_are_quoted_when_they_must_be() {
        let table = rows(&[&["name", "note"], &["Doe, Jane", "said \"hi\""], &["x", "two\nlines"], &[""], &["a", ""]]);
        let text = sheets_format_csv(table.clone(), ",".into()).unwrap();
        assert_eq!(text, "name,note\n\"Doe, Jane\",\"said \"\"hi\"\"\"\nx,\"two\nlines\"\n\"\"\na,\n");
        // what is written reads back the same
        assert_eq!(parse(&text, ",").unwrap(), table);
    }

    #[test]
    fn other_delimiters() {
        let table = rows(&[&["a;b", "c,d"], &["1"]]);
        let text = sheets_format_csv(table.clone(), ";".into()).unwrap();
        assert_eq!(text, "\"a;b\";c,d\n1\n");
        assert_eq!(parse(&text, ";").unwrap(), table);
        assert_eq!(sheets_format_csv(Vec::new(), "\t".into()).unwrap(), "");
        assert!(sheets_format_csv(table, "\"".into()).unwrap_err().contains("cannot be a delimiter"));
    }

    #[test]
    fn files_are_written() {
        let path = scratch::text("written.csv");
        sheets_write_csv(path.clone(), rows(&[&["id", "label"], &["1", "un, deux"]]), ",".into()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "id,label\n1,\"un, deux\"\n");
        let nowhere = scratch::text("no/such/dir/x.csv");
        let e = sheets_write_csv(nowhere.clone(), Vec::new(), ",".into()).unwrap_err();
        assert!(e.starts_with(&format!("cannot write {nowhere}")), "{e}");
        assert!(sheets_write_csv(path, Vec::new(), "ab".into()).is_err());
    }
}
