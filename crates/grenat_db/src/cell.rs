//! A value in a database: what a parameter or a column holds.

#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    /// Bytes: a `BLOB` (SQLite), a `BYTEA` (PostgreSQL).
    Blob(Vec<u8>),
}
