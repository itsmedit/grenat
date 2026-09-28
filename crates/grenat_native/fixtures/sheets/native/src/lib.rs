//! The native part of the test facet `sheets`: pure and effectful
//! functions, a struct, errors, a panic, optional values, arrays and maps.

use std::collections::HashMap;

use grenat_ext::{GrenatType, export};
use serde::{Deserialize, Serialize};

/// A cell of a sheet.
#[derive(Serialize, Deserialize, GrenatType)]
pub struct Cell {
    /// Its row, from 0.
    pub row: i64,
    pub col: i64,
    pub text: String,
}

/// Adds two integers.
#[export(pure)]
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// The sum of the numbers.
#[export(pure)]
pub fn sum(xs: Vec<i64>) -> i64 {
    xs.iter().sum()
}

/// The text in capitals: not pure, so its result is untrusted.
#[export]
pub fn shout(text: String) -> String {
    text.to_uppercase()
}

/// Reads a sheet: a line per row, cells separated by commas.
#[export(effects = "fs.read", error = "SheetError")]
pub fn read_sheet(path: String) -> Result<Vec<Vec<String>>, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    Ok(text.lines().map(|line| line.split(',').map(|cell| cell.trim().to_string()).collect()).collect())
}

/// The cells of the rows.
#[export(pure)]
pub fn cells(rows: Vec<Vec<String>>) -> Vec<Cell> {
    let mut cells = Vec::new();
    for (row, texts) in rows.into_iter().enumerate() {
        for (col, text) in texts.into_iter().enumerate() {
            cells.push(Cell { row: row as i64, col: col as i64, text });
        }
    }
    cells
}

/// The cell with the longest text.
#[export(pure)]
pub fn longest(cells: Vec<Cell>) -> Option<Cell> {
    cells.into_iter().max_by_key(|c| c.text.len())
}

/// Where `word` is among `words`.
#[export(pure)]
pub fn find(words: Vec<String>, word: String) -> Option<i64> {
    words.iter().position(|w| *w == word).map(|i| i as i64)
}

/// How many times each word appears.
#[export(pure)]
pub fn count_words(text: String) -> HashMap<String, i64> {
    let mut counts = HashMap::new();
    for word in text.split_whitespace() {
        *counts.entry(word.to_string()).or_insert(0) += 1;
    }
    counts
}

/// The mean of the numbers, if there are any.
#[export(pure)]
pub fn mean(xs: Vec<f64>) -> Option<f64> {
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

/// `a / b`: an error when `b` is 0.
#[export(pure)]
pub fn ratio(a: f64, b: f64) -> Result<f64, String> {
    if b == 0.0 { Err(format!("cannot divide {a} by zero")) } else { Ok(a / b) }
}

/// Pretends to fetch a URL (a `net` effect, whose argument must be trusted).
#[export(effects = "net")]
pub fn fetch(url: String) -> String {
    format!("fetched {url}")
}

/// Panics, always.
#[export]
pub fn explode(message: String) -> bool {
    panic!("{message}")
}
