//! Columns added to a table after its first release: a database made by an
//! earlier Grenat gets them when the table is next used, and keeps its rows.
//!
//! The columns are read from the catalog (never by a query that fails: on
//! PostgreSQL, a failed statement would abort the transaction the program
//! may be in), and each one missing is added with its default.

use grenat_db::{Cell, Connection, Dialect};

use crate::Result;
use crate::row::text;

/// Adds to `table` those of `columns` — `(name, definition)` — it lacks.
pub(crate) fn add_missing(db: &mut dyn Connection, table: &str, columns: &[(&str, &str)]) -> Result<()> {
    let present = names(db, table)?;
    for (name, definition) in columns {
        if present.iter().any(|p| p == name) {
            continue;
        }
        let added = match db.dialect() {
            // another process may add it at the same time
            Dialect::Postgres => db.batch(&format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {name} {definition}")),
            Dialect::Sqlite => db.batch(&format!("ALTER TABLE {table} ADD COLUMN {name} {definition}")),
        };
        match added {
            Err(e) if e.contains("duplicate column") => {}
            other => other?,
        }
    }
    Ok(())
}

/// The columns of `table`, as an unqualified name finds it.
fn names(db: &mut dyn Connection, table: &str) -> Result<Vec<String>> {
    let sql = match db.dialect() {
        Dialect::Sqlite => "SELECT name FROM pragma_table_info(?)",
        Dialect::Postgres => {
            "SELECT attname::text FROM pg_attribute WHERE attrelid = to_regclass(?::text) AND attnum > 0 AND NOT attisdropped"
        }
    };
    let rows = db.query(sql, &[Cell::Text(table.to_string())])?;
    Ok(rows.iter().map(|r| text(r, 0)).collect())
}
