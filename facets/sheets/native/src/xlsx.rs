//! Writing `.xlsx` workbooks with rust_xlsxwriter: sheets of rows of text.
//!
//! Every cell is written as text, exactly as given — `007` stays `007`,
//! and `=SUM(A1:A2)` is text, never a formula — unless the writing is
//! `typed`: then a cell that is a number ([`crate::number`]: one that
//! reads back as the same text, such as `42` or `-3.5`) is written as a
//! number, which a spreadsheet sums and sorts as one. Either way, reading
//! the file back gives the rows' used range ([`crate::workbook`]): the
//! same rows when they are all as wide and their first and last rows and
//! columns each have a non-empty cell; otherwise empty rows and columns at
//! the edges are dropped, and shorter rows padded with `""`.

use grenat_ext::{GrenatType, export};
use rust_xlsxwriter::Workbook;
use serde::{Deserialize, Serialize};

use crate::number::number;

/// A sheet to write: its name and its rows of cells.
#[derive(Serialize, Deserialize, GrenatType, Debug, Clone, PartialEq)]
pub struct Worksheet {
    /// The sheet's name: at most 31 characters, none of `[]:*?/\`.
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

/// Writes a workbook of the sheets (one at least) to the `.xlsx` file at
/// `path`, replacing it; `typed`, numbers are written as numbers.
#[export(effects = "fs.write", error = "SheetError")]
pub fn sheets_write_xlsx(path: String, sheets: Vec<Worksheet>, typed: bool) -> Result<(), String> {
    let bytes = workbook(&sheets, typed)?;
    std::fs::write(&path, bytes).map_err(|e| format!("cannot write {path}: {e}"))
}

/// The bytes of the workbook.
fn workbook(sheets: &[Worksheet], typed: bool) -> Result<Vec<u8>, String> {
    if sheets.is_empty() {
        return Err("a workbook has one sheet at least".to_string());
    }
    let mut book = Workbook::new();
    for sheet in sheets {
        let out = book.add_worksheet();
        out.set_name(&sheet.name).map_err(|e| format!("the sheet name `{}`: {e}", sheet.name))?;
        for (r, row) in sheet.rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let (row, col) = position(r, c, &sheet.name)?;
                let written = match number(cell) {
                    Some(value) if typed => out.write_number(row, col, value).map(drop),
                    _ if cell.is_empty() => Ok(()),
                    _ => out.write_string(row, col, cell).map(drop),
                };
                written.map_err(|e| format!("the sheet `{}`, row {}, column {}: {e}", sheet.name, r + 1, c + 1))?;
            }
        }
    }
    book.save_to_buffer().map_err(|e| e.to_string())
}

/// The cell at row `r` and column `c` (from 0), within Excel's limits.
fn position(r: usize, c: usize, sheet: &str) -> Result<(u32, u16), String> {
    const ROWS: usize = 1_048_576;
    const COLUMNS: usize = 16_384;
    if r >= ROWS || c >= COLUMNS {
        return Err(format!("the sheet `{sheet}` is too large: Excel keeps {ROWS} rows of {COLUMNS} cells at most"));
    }
    Ok((r as u32, c as u16))
}

#[cfg(test)]
mod tests {
    use calamine::{Data, Reader, open_workbook_auto};

    use super::*;
    use crate::scratch;
    use crate::workbook::{sheets_names, sheets_rows};

    fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|row| row.iter().map(|c| c.to_string()).collect()).collect()
    }

    fn sheets() -> Vec<Worksheet> {
        vec![
            Worksheet {
                name: "Prices".into(),
                rows: rows(&[&["item", "price", "code"], &["pen", "1.5", "007"], &["=SUM(B2:B2)", "42", "1.50"]]),
            },
            Worksheet { name: "Notes".into(), rows: rows(&[&["two\nlines", "", "\"quoted\", with a comma"]]) },
        ]
    }

    /// The cell at row `r`, column `c` of the sheet `name`, as the file keeps it.
    fn cell(path: &str, name: &str, r: u32, c: u32) -> Data {
        let mut book = open_workbook_auto(path).unwrap();
        book.worksheet_range(name).unwrap().get_value((r, c)).cloned().unwrap_or(Data::Empty)
    }

    #[test]
    fn text_is_written_as_text_and_reads_back_the_same() {
        let dir = scratch::Dir::new();
        let path = dir.text("text.xlsx");
        sheets_write_xlsx(path.clone(), sheets(), false).unwrap();
        assert_eq!(sheets_names(path.clone()).unwrap(), ["Prices", "Notes"]);
        assert_eq!(sheets_rows(path.clone(), None).unwrap(), sheets()[0].rows);
        assert_eq!(sheets_rows(path.clone(), Some("Notes".into())).unwrap(), sheets()[1].rows);
        assert_eq!(cell(&path, "Prices", 2, 1), Data::String("42".into()));
        assert_eq!(cell(&path, "Prices", 2, 0), Data::String("=SUM(B2:B2)".into()));
    }

    #[test]
    fn typed_writing_writes_numbers_and_keeps_the_rest() {
        let dir = scratch::Dir::new();
        let path = dir.text("typed.xlsx");
        sheets_write_xlsx(path.clone(), sheets(), true).unwrap();
        assert_eq!(cell(&path, "Prices", 2, 1), Data::Float(42.0));
        assert_eq!(cell(&path, "Prices", 1, 1), Data::Float(1.5));
        assert_eq!(cell(&path, "Prices", 1, 2), Data::String("007".into()));
        assert_eq!(cell(&path, "Prices", 2, 2), Data::String("1.50".into()));
        assert_eq!(sheets_rows(path, None).unwrap(), sheets()[0].rows);
    }

    #[test]
    fn reading_back_gives_the_used_range() {
        let dir = scratch::Dir::new();
        let back = |name: &str, written: &[&[&str]]| {
            let path = dir.text(name);
            let sheet = Worksheet { name: "S".into(), rows: rows(written) };
            sheets_write_xlsx(path.clone(), vec![sheet], false).unwrap();
            sheets_rows(path, None).unwrap()
        };
        // as wide, with a non-empty cell in the first and last rows and columns
        let same: &[&[&str]] = &[&["a", "", ""], &["", "", "b"], &["c", "", ""]];
        assert_eq!(back("same.xlsx", same), rows(same));
        // empty rows and columns at the edges are dropped, shorter rows padded
        assert_eq!(back("column.xlsx", &[&["", "a"], &["", "b"]]), rows(&[&["a"], &["b"]]));
        assert_eq!(back("row.xlsx", &[&[""], &["x"]]), rows(&[&["x"]]));
        assert_eq!(back("trailing.xlsx", &[&["a", ""], &["", ""]]), rows(&[&["a"]]));
        assert_eq!(back("ragged.xlsx", &[&["a", "b"], &["c"]]), rows(&[&["a", "b"], &["c", ""]]));
    }

    #[test]
    fn wrong_workbooks_are_errors() {
        let dir = scratch::Dir::new();
        let path = dir.text("wrong.xlsx");
        assert_eq!(
            sheets_write_xlsx(path.clone(), Vec::new(), false).unwrap_err(),
            "a workbook has one sheet at least"
        );
        let bad = Worksheet { name: "a/b".into(), rows: Vec::new() };
        assert!(sheets_write_xlsx(path.clone(), vec![bad], false).unwrap_err().starts_with("the sheet name `a/b`: "));
        let twice =
            vec![Worksheet { name: "A".into(), rows: Vec::new() }, Worksheet { name: "a".into(), rows: Vec::new() }];
        assert!(sheets_write_xlsx(path.clone(), twice, false).is_err());
        let long = Worksheet { name: "L".into(), rows: vec![vec!["x".repeat(40_000)]] };
        let e = sheets_write_xlsx(path, vec![long], false).unwrap_err();
        assert!(e.starts_with("the sheet `L`, row 1, column 1: "), "{e}");
        let nowhere = dir.text("no/such/dir/x.xlsx");
        let e = sheets_write_xlsx(nowhere.clone(), sheets(), false).unwrap_err();
        assert!(e.starts_with(&format!("cannot write {nowhere}")), "{e}");
        assert!(position(1_048_576, 0, "S").unwrap_err().contains("too large"));
        assert!(position(0, 16_384, "S").is_err());
        assert_eq!(position(1_048_575, 16_383, "S"), Ok((1_048_575, 16_383)));
    }
}
