//! Rows as records: the first row names the columns, each following row
//! becomes a hash from those names to its cells.
//!
//! - a column name appears once (else the error names it);
//! - a row with fewer cells than names gets `""` for the missing ones;
//!   one with more fails, unless the extra cells are empty;
//! - a row whose cells are all empty (a blank line in a sheet) is skipped.

use std::collections::BTreeMap;

/// A record: column name → cell.
pub type Record = BTreeMap<String, String>;

/// The records of `rows`, whose first row is the header.
pub fn records(rows: Vec<Vec<String>>) -> Result<Vec<Record>, String> {
    let mut rows = rows.into_iter();
    let Some(headers) = rows.next() else { return Ok(Vec::new()) };
    for (i, name) in headers.iter().enumerate() {
        if headers[..i].contains(name) {
            return Err(format!("the column `{name}` appears twice in the header row"));
        }
    }
    let mut records = Vec::new();
    // rows are counted from 1, the header being row 1
    for (number, row) in rows.enumerate().map(|(i, row)| (i + 2, row)) {
        if row.iter().all(String::is_empty) {
            continue;
        }
        if row[headers.len().min(row.len())..].iter().any(|cell| !cell.is_empty()) {
            return Err(format!("row {number} has {} cells, the header row {}", row.len(), headers.len()));
        }
        let mut cells = row.into_iter();
        records.push(headers.iter().map(|name| (name.clone(), cells.next().unwrap_or_default())).collect());
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|row| row.iter().map(|c| c.to_string()).collect()).collect()
    }

    fn record(pairs: &[(&str, &str)]) -> Record {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn the_first_row_names_the_columns() {
        let got = records(rows(&[&["name", "age"], &["Ada", "36"], &["Linus"], &["", ""], &["Matz", "59", ""]]));
        assert_eq!(
            got.unwrap(),
            [
                record(&[("name", "Ada"), ("age", "36")]),
                record(&[("name", "Linus"), ("age", "")]),
                record(&[("name", "Matz"), ("age", "59")]),
            ]
        );
        assert_eq!(records(Vec::new()).unwrap(), Vec::<Record>::new());
        assert_eq!(records(rows(&[&["only", "headers"]])).unwrap(), Vec::<Record>::new());
    }

    #[test]
    fn ambiguous_tables_are_refused() {
        let e = records(rows(&[&["a", "b", "a"], &["1", "2", "3"]])).unwrap_err();
        assert_eq!(e, "the column `a` appears twice in the header row");
        let e = records(rows(&[&["a", "b"], &["1", "2"], &["1", "2", "3"]])).unwrap_err();
        assert_eq!(e, "row 3 has 3 cells, the header row 2");
    }
}
