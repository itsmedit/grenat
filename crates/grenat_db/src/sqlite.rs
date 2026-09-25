//! SQLite, embedded (the library is compiled in).

use rusqlite::types::{Value, ValueRef};

use crate::{Cell, Connection, Row};

pub(crate) struct Sqlite(rusqlite::Connection);

impl Sqlite {
    pub(crate) fn open(path: &str) -> Result<Sqlite, String> {
        rusqlite::Connection::open(path).map(Sqlite).map_err(|e| format!("cannot open {path}: {e}"))
    }

    pub(crate) fn memory() -> Result<Sqlite, String> {
        rusqlite::Connection::open_in_memory().map(Sqlite).map_err(|e| e.to_string())
    }
}

fn value(cell: &Cell) -> Value {
    match cell {
        Cell::Null => Value::Null,
        Cell::Bool(b) => Value::Integer(i64::from(*b)),
        Cell::Int(n) => Value::Integer(*n),
        Cell::Float(f) => Value::Real(*f),
        Cell::Text(s) => Value::Text(s.clone()),
    }
}

fn cell(value: ValueRef) -> Cell {
    match value {
        ValueRef::Null => Cell::Null,
        ValueRef::Integer(n) => Cell::Int(n),
        ValueRef::Real(f) => Cell::Float(f),
        ValueRef::Text(t) | ValueRef::Blob(t) => Cell::Text(String::from_utf8_lossy(t).into_owned()),
    }
}

impl Connection for Sqlite {
    fn dialect(&self) -> crate::Dialect {
        crate::Dialect::Sqlite
    }

    fn query(&mut self, sql: &str, params: &[Cell]) -> Result<Vec<Row>, String> {
        let mut statement = self.0.prepare(sql).map_err(|e| e.to_string())?;
        let names: Vec<String> = statement.column_names().iter().map(|n| n.to_string()).collect();
        let values: Vec<Value> = params.iter().map(value).collect();
        let mut rows = statement.query(rusqlite::params_from_iter(values)).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let mut cells = Vec::with_capacity(names.len());
            for (i, name) in names.iter().enumerate() {
                cells.push((name.clone(), cell(row.get_ref(i).map_err(|e| e.to_string())?)));
            }
            out.push(cells);
        }
        Ok(out)
    }

    fn execute(&mut self, sql: &str, params: &[Cell]) -> Result<u64, String> {
        let values: Vec<Value> = params.iter().map(value).collect();
        self.0.execute(sql, rusqlite::params_from_iter(values)).map(|n| n as u64).map_err(|e| e.to_string())
    }

    fn batch(&mut self, sql: &str) -> Result<(), String> {
        self.0.execute_batch(sql).map_err(|e| e.to_string())
    }
}
