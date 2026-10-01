//! Reading workbooks — `.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.ods`, known by
//! their extension (else by their content) — with calamine.
//!
//! A sheet's rows are its used range: from its first non-empty row and
//! column to its last, every row as wide as the widest, each cell as text
//! ([`crate::cells`]), an empty one `""`. Formulas give the value the file
//! saved for them.
//!
//! The used range is measured before it is built ([`crate::grid`]): a sheet
//! whose cells spread over more than [`MAX_CELLS`](crate::grid::MAX_CELLS)
//! cells is refused, however small its file. An `.xlsx`, `.xlsm` or `.xlsb`
//! sheet is read cell by cell, and refused as soon as its cells spread too
//! far; calamine reads an `.xls` or `.ods` workbook whole when it opens it
//! (capping an `.ods` sheet at 100 million cells), and its sheet is
//! measured before its cells become texts.

use calamine::{Data, Range, Reader, Sheets, open_workbook_auto};
use grenat_ext::export;

use crate::cells;
use crate::grid::{self, Grid, MAX_CELLS};
use crate::records::{Record, records};

/// The names of the sheets of a workbook, in order.
#[export(effects = "fs.read", error = "SheetError")]
pub fn sheets_names(path: String) -> Result<Vec<String>, String> {
    Ok(open(&path)?.sheet_names())
}

/// The rows of a sheet of a workbook (its first sheet when `sheet` is
/// nil), each cell as text.
#[export(effects = "fs.read", error = "SheetError")]
pub fn sheets_rows(path: String, sheet: Option<String>) -> Result<Vec<Vec<String>>, String> {
    rows(&path, sheet)
}

/// The records of a sheet of a workbook (its first sheet when `sheet` is
/// nil), whose first row names the columns.
#[export(effects = "fs.read", error = "SheetError")]
pub fn sheets_records(path: String, sheet: Option<String>) -> Result<Vec<Record>, String> {
    records(rows(&path, sheet)?).map_err(|e| format!("{path}: {e}"))
}

fn open(path: &str) -> Result<Sheets<std::io::BufReader<std::fs::File>>, String> {
    open_workbook_auto(path).map_err(|e| format!("cannot read the workbook {path}: {e}"))
}

fn rows(path: &str, sheet: Option<String>) -> Result<Vec<Vec<String>>, String> {
    let mut book = open(path)?;
    let names = book.sheet_names();
    let name = match sheet {
        Some(name) if names.contains(&name) => name,
        Some(name) => return Err(format!("{path} has no sheet named `{name}`: its sheets are {}", list(&names))),
        None => names.first().cloned().ok_or_else(|| format!("{path} has no sheet"))?,
    };
    sheet_rows(&mut book, &name).map_err(|e| format!("cannot read the sheet `{name}` of {path}: {e}"))
}

/// The rows of the sheet `name` of `book`, within the budget of cells.
fn sheet_rows<RS: std::io::Read + std::io::Seek>(
    book: &mut Sheets<RS>,
    name: &str,
) -> Result<Vec<Vec<String>>, String> {
    let mut grid = Grid::new(MAX_CELLS);
    match book {
        Sheets::Xlsx(xlsx) => {
            let mut reader = match xlsx.worksheet_cells_reader(name) {
                Ok(reader) => reader,
                // a chart sheet: no cells (as calamine reads it)
                Err(calamine::XlsxError::NotAWorksheet(_)) => return Ok(Vec::new()),
                Err(e) => return Err(e.to_string()),
            };
            while let Some(cell) = reader.next_cell().map_err(|e| e.to_string())? {
                add(&mut grid, cell)?;
            }
        }
        Sheets::Xlsb(xlsb) => {
            let mut reader = xlsb.worksheet_cells_reader(name).map_err(|e| e.to_string())?;
            while let Some(cell) = reader.next_cell().map_err(|e| e.to_string())? {
                add(&mut grid, cell)?;
            }
        }
        Sheets::Xls(_) | Sheets::Ods(_) => {
            return texts(&book.worksheet_range(name).map_err(|e| e.to_string())?, MAX_CELLS);
        }
    }
    Ok(grid.rows())
}

/// Adds a cell read from a file to `grid`, unless it is empty (as calamine
/// leaves it out of a sheet's range).
fn add(grid: &mut Grid, cell: calamine::Cell<calamine::DataRef<'_>>) -> Result<(), String> {
    if *cell.get_value() == calamine::DataRef::Empty {
        return Ok(());
    }
    grid.add(cell.get_position(), cells::text(&Data::from(cell.get_value().clone())))
}

/// The cells of `range`, as text, row by row, if it spans `budget` cells
/// at most.
fn texts(range: &Range<Data>, budget: u64) -> Result<Vec<Vec<String>>, String> {
    grid::check(range.height(), range.width(), budget)?;
    Ok(range.rows().map(|row| row.iter().map(cells::text).collect()).collect())
}

fn list(names: &[String]) -> String {
    names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};

    use super::*;
    use crate::scratch;

    /// A workbook with two sheets: text, numbers, a boolean, a date, a
    /// formula, empty cells, a table that does not start at A1.
    fn workbook(dir: &scratch::Dir, name: &str) -> String {
        let path = dir.text(name);
        let mut book = Workbook::new();
        let people = book.add_worksheet().set_name("People").unwrap();
        people.write_string(0, 0, "name").unwrap();
        people.write_string(0, 1, "age").unwrap();
        people.write_string(0, 2, "joined").unwrap();
        people.write_string(1, 0, "Ada, \"the first\"").unwrap();
        people.write_number(1, 1, 36).unwrap();
        let date = ExcelDateTime::from_ymd(2024, 1, 15).unwrap();
        people.write_datetime_with_format(1, 2, &date, &Format::new().set_num_format("yyyy-mm-dd")).unwrap();
        people.write_string(2, 0, "Linus").unwrap();
        people.write_number(2, 1, 2.5).unwrap();
        people.write_boolean(2, 2, true).unwrap();
        let totals = book.add_worksheet().set_name("Totals").unwrap();
        totals.write_string(2, 1, "total").unwrap();
        totals.write_number(3, 1, 3).unwrap();
        totals.write_formula(4, 1, "=B4*2").unwrap();
        book.save(&path).unwrap();
        path
    }

    /// An OpenDocument spreadsheet, written by hand: a sheet of two rows.
    fn ods(dir: &scratch::Dir, name: &str) -> String {
        let path = dir.text(name);
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.oasis.opendocument.spreadsheet").unwrap();
        zip.start_file("META-INF/manifest.xml", stored).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2">
<manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/>
<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
</manifest:manifest>"#,
        )
        .unwrap();
        zip.start_file("content.xml", stored).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2">
<office:body><office:spreadsheet><table:table table:name="Stock">
<table:table-row><table:table-cell office:value-type="string"><text:p>item</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>count</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell office:value-type="string"><text:p>pens</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="12"><text:p>12</text:p></table:table-cell></table:table-row>
</table:table></office:spreadsheet></office:body></office:document-content>"#,
        )
        .unwrap();
        zip.finish().unwrap();
        path
    }

    fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|row| row.iter().map(|c| c.to_string()).collect()).collect()
    }

    #[test]
    fn sheets_are_named_in_order() {
        let dir = scratch::Dir::new();
        let path = workbook(&dir, "names.xlsx");
        assert_eq!(sheets_names(path).unwrap(), ["People", "Totals"]);
        assert_eq!(sheets_names(ods(&dir, "names.ods")).unwrap(), ["Stock"]);
    }

    #[test]
    fn rows_are_text() {
        let dir = scratch::Dir::new();
        let path = workbook(&dir, "rows.xlsx");
        let people =
            rows(&[&["name", "age", "joined"], &["Ada, \"the first\"", "36", "2024-01-15"], &["Linus", "2.5", "true"]]);
        assert_eq!(sheets_rows(path.clone(), None).unwrap(), people);
        assert_eq!(sheets_rows(path.clone(), Some("People".into())).unwrap(), people);
        // the used range, from B3; a formula is its saved value (none here: rust_xlsxwriter saves 0)
        assert_eq!(sheets_rows(path, Some("Totals".into())).unwrap(), rows(&[&["total"], &["3"], &["0"]]));
        assert_eq!(sheets_rows(ods(&dir, "rows.ods"), None).unwrap(), rows(&[&["item", "count"], &["pens", "12"]]));
    }

    #[test]
    fn records_use_the_header_row() {
        let dir = scratch::Dir::new();
        let got = sheets_records(workbook(&dir, "records.xlsx"), None).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["name"], "Ada, \"the first\"");
        assert_eq!(got[1]["joined"], "true");
        let got = sheets_records(ods(&dir, "records.ods"), Some("Stock".into())).unwrap();
        assert_eq!(got[0]["count"], "12");
    }

    /// A workbook of two cells: `a` at A1, `z` at `(row, column)`.
    fn far_apart(dir: &scratch::Dir, name: &str, row: u32, column: u16) -> String {
        let path = dir.text(name);
        let mut book = Workbook::new();
        let sheet = book.add_worksheet().set_name("Far").unwrap();
        sheet.write_string(0, 0, "a").unwrap();
        sheet.write_string(row, column, "z").unwrap();
        book.save(&path).unwrap();
        path
    }

    #[test]
    fn a_small_file_whose_cells_spread_too_far_is_refused() {
        let dir = scratch::Dir::new();
        // a few kilobytes, 17 billion cells between A1 and XFD1048576
        let path = far_apart(&dir, "farthest.xlsx", 1_048_575, 16_383);
        assert!(std::fs::metadata(&path).unwrap().len() < 10_000);
        let e = sheets_rows(path.clone(), None).unwrap_err();
        assert_eq!(
            e,
            format!(
                "cannot read the sheet `Far` of {path}: its cells spread over 1048576 rows of 16384 columns at least: \
                 more than the 10000000 cells a sheet may span"
            )
        );
        assert!(sheets_records(path.clone(), None).unwrap_err().contains("more than the 10000000 cells"));
        // the names of its sheets are still read
        assert_eq!(sheets_names(path).unwrap(), ["Far"]);
        // 2000 rows of 16384 columns: 32 million cells
        let path = far_apart(&dir, "wide.xlsx", 1999, 16_383);
        assert!(
            sheets_rows(path, None)
                .unwrap_err()
                .ends_with("2000 rows of 16384 columns at least: more than the 10000000 cells a sheet may span")
        );
        // within the budget, the box between the cells is read
        let path = far_apart(&dir, "near.xlsx", 2, 3);
        assert_eq!(
            sheets_rows(path, None).unwrap(),
            rows(&[&["a", "", "", ""], &["", "", "", ""], &["", "", "", "z"]])
        );
    }

    #[test]
    fn a_sheet_read_whole_is_measured_before_its_cells_become_text() {
        let small = Range::from_sparse(vec![
            calamine::Cell::new((1, 1), Data::Int(1)),
            calamine::Cell::new((2, 2), Data::Bool(true)),
        ]);
        assert_eq!(texts(&small, 4).unwrap(), rows(&[&["1", ""], &["", "true"]]));
        assert_eq!(
            texts(&small, 3).unwrap_err(),
            "its cells spread over 2 rows of 2 columns: more than the 3 cells a sheet may span"
        );
    }

    #[test]
    fn missing_sheets_and_files_are_errors() {
        let dir = scratch::Dir::new();
        let path = workbook(&dir, "errors.xlsx");
        let e = sheets_rows(path.clone(), Some("Nope".into())).unwrap_err();
        assert_eq!(e, format!("{path} has no sheet named `Nope`: its sheets are `People`, `Totals`"));
        assert!(sheets_records(path, Some("Nope".into())).is_err());
        let missing = dir.text("missing.xlsx");
        assert!(sheets_names(missing.clone()).unwrap_err().starts_with(&format!("cannot read the workbook {missing}")));
        let junk = dir.text("junk.xlsx");
        std::fs::write(&junk, "not a workbook").unwrap();
        assert!(sheets_rows(junk.clone(), None).unwrap_err().starts_with(&format!("cannot read the workbook {junk}")));
        let unknown = dir.text("junk.bin");
        std::fs::write(&unknown, "not a workbook either").unwrap();
        assert!(sheets_names(unknown).is_err());
    }
}
