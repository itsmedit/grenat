//! Reading the cells of a row.

use grenat_db::{Cell, Row};

fn cell(row: &Row, i: usize) -> &Cell {
    row.get(i).map_or(&Cell::Null, |(_, c)| c)
}

pub(crate) fn text(row: &Row, i: usize) -> String {
    opt_text(row, i).unwrap_or_default()
}

pub(crate) fn opt_text(row: &Row, i: usize) -> Option<String> {
    match cell(row, i) {
        Cell::Text(t) => Some(t.clone()),
        _ => None,
    }
}

pub(crate) fn int(row: &Row, i: usize) -> i64 {
    opt_int(row, i).unwrap_or(0)
}

pub(crate) fn opt_int(row: &Row, i: usize) -> Option<i64> {
    match cell(row, i) {
        Cell::Int(n) => Some(*n),
        Cell::Float(f) => Some(*f as i64),
        _ => None,
    }
}

pub(crate) fn float(row: &Row, i: usize) -> f64 {
    opt_float(row, i).unwrap_or(0.0)
}

pub(crate) fn opt_float(row: &Row, i: usize) -> Option<f64> {
    match cell(row, i) {
        Cell::Float(f) => Some(*f),
        Cell::Int(n) => Some(*n as f64),
        _ => None,
    }
}

pub(crate) fn boolean(row: &Row, i: usize) -> bool {
    match cell(row, i) {
        Cell::Bool(b) => *b,
        Cell::Int(n) => *n != 0,
        _ => false,
    }
}

/// `Some(text)` as a cell, `None` as NULL.
pub(crate) fn nullable(text: Option<&str>) -> Cell {
    text.map_or(Cell::Null, |t| Cell::Text(t.to_string()))
}
