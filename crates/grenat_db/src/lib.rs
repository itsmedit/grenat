//! Databases: SQLite and PostgreSQL behind one [`Connection`] interface.
//!
//! Queries are always parameterized, with `?` placeholders on every
//! database (they become `$1`, `$2`… for PostgreSQL): a value never becomes
//! SQL text. Values in and out are [`Cell`]s; the language maps them to its
//! own values.

mod cell;
mod placeholders;
mod postgres_db;
mod sqlite;

pub use cell::Cell;

/// A row: (column, value), in the order of the query's columns.
pub type Row = Vec<(String, Cell)>;

/// Which SQL a database speaks, where it differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    Postgres,
}

impl Dialect {
    /// The column type of an id the database gives.
    pub fn primary_key(self) -> &'static str {
        match self {
            Dialect::Sqlite => "INTEGER PRIMARY KEY",
            Dialect::Postgres => "BIGSERIAL PRIMARY KEY",
        }
    }
}

pub trait Connection: Send {
    fn dialect(&self) -> Dialect;
    /// The rows of a query.
    fn query(&mut self, sql: &str, params: &[Cell]) -> Result<Vec<Row>, String>;
    /// Runs a statement; the number of rows it changed.
    fn execute(&mut self, sql: &str, params: &[Cell]) -> Result<u64, String>;
    /// Runs statements without parameters, separated by `;` (a migration).
    fn batch(&mut self, sql: &str) -> Result<(), String>;
}

/// Opens `url`: `sqlite://path/to/file.db`, `sqlite::memory:`, or
/// `postgres://user:password@host:port/database`.
pub fn connect(url: &str) -> Result<Box<dyn Connection>, String> {
    if url == "sqlite::memory:" {
        return sqlite::Sqlite::memory().map(|c| Box::new(c) as Box<dyn Connection>);
    }
    if let Some(path) = url.strip_prefix("sqlite://") {
        return sqlite::Sqlite::open(path).map(|c| Box::new(c) as Box<dyn Connection>);
    }
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        return postgres_db::Postgres::connect(url).map(|c| Box::new(c) as Box<dyn Connection>);
    }
    Err(format!("unsupported database URL `{url}` (expected sqlite://…, sqlite::memory: or postgres://…)"))
}
