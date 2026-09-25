//! `Db`: databases in the standard library (see `grenat_db`).
//!
//! ```ruby
//! db = Db.connect("sqlite://app.db")          # or postgres://…
//! db.migrate("CREATE TABLE orders (id INTEGER PRIMARY KEY, total FLOAT)")
//! db.execute("INSERT INTO orders (total) VALUES (?)", [9.5])
//! db.query("SELECT * FROM orders WHERE total > ?", [5])      # hashes
//! db.query("SELECT * FROM orders", as: Order)                 # records
//! db.first("SELECT * FROM orders WHERE id = ?", [1], as: Order)
//! db.transaction do … end
//! ```
//!
//! Reads are `db.read` effects, writes `db.write`, checked against the
//! capabilities at run time. SQL text is never untrusted (a `TaintError`
//! otherwise): values go in parameters. Untrusted values may filter a read,
//! never be written.

use grenat_db::Cell;

use crate::prelude::*;

use super::*;

/// The record `Db.connect` returns.
pub(crate) const DATABASE: &str = "Database";

pub(crate) fn connect<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    let url = str_arg(args, 0, "connect")?.to_string();
    let connection = grenat_green::blocking(|| grenat_db::connect(&url)).or_else(db_error)?;
    Ok(interp.register_database(connection))
}

impl<'p> Interp<'p> {
    /// A connection, as the `Database` record programs use.
    pub(crate) fn register_database(&self, connection: Box<dyn grenat_db::Connection>) -> Value<'p> {
        let mut databases = self.databases.borrow_mut();
        databases.push(Arc::new(grenat_green::Mutex::new(connection)));
        Value::record(DATABASE, vec![("id".into(), Value::Int(databases.len() as i64 - 1))])
    }

    /// The connection of a `Database` record.
    pub(crate) fn connection_of(&self, database: &Value<'p>) -> Option<crate::SharedConnection> {
        match database.untainted() {
            Value::Record(r) if &*r.ty == DATABASE => match r.fields.first() {
                Some((_, Value::Int(id))) => self.databases.borrow().get(*id as usize).cloned(),
                _ => None,
            },
            _ => None,
        }
    }
}

/// The methods of a database.
pub(crate) fn database_method<'p>(interp: &mut Interp<'p>, fields: &Fields<'p>, name: &str, args: Args<'p>) -> R<'p> {
    let id = match fields.first().map(|(_, v)| v) {
        Some(Value::Int(n)) => *n as usize,
        _ => return raise("DbError", "not a database"),
    };
    let connection = interp.databases.borrow()[id].clone();
    let sql = |args: &Args<'p>| -> Result<String, Ctrl<'p>> {
        let text = arg(args, 0, name)?;
        if text.contains_taint() {
            return raise("TaintError", format!("an untrusted value is SQL text in `{name}`: pass values as parameters"));
        }
        Ok(text.to_display())
    };
    match name {
        "query" | "first" => {
            interp.check_effect("db.read")?;
            let (sql, params) = (sql(&args)?, params(&args, false)?);
            let rows = grenat_green::blocking(|| connection.lock().query(&sql, &params)).or_else(db_error)?;
            let record = match args.named.iter().find(|(n, _)| n == "as") {
                Some((_, Value::Type(ty))) => Some(ty.clone()),
                Some((_, other)) => return raise("TypeError", format!("`as:` expects a type, got {}", other.inspect())),
                None => None,
            };
            let take = if name == "first" { 1 } else { rows.len() };
            let mut values = Vec::new();
            for row in rows.into_iter().take(take) {
                values.push(row_value(interp, row, record.as_deref())?);
            }
            Ok(if name == "first" { values.pop().unwrap_or(Value::Nil) } else { Value::array(values) })
        }
        "execute" => {
            interp.check_effect("db.write")?;
            let (sql, params) = (sql(&args)?, params(&args, true)?);
            let changed = grenat_green::blocking(|| connection.lock().execute(&sql, &params)).or_else(db_error)?;
            Ok(Value::Int(changed as i64))
        }
        "migrate" => {
            interp.check_effect("db.write")?;
            let sql = sql(&args)?;
            grenat_green::blocking(|| connection.lock().batch(&sql)).or_else(db_error)?;
            Ok(Value::Nil)
        }
        "transaction" => {
            interp.check_effect("db.write")?;
            let body = block(&args, name)?;
            let run = |sql: &str| grenat_green::blocking(|| connection.lock().batch(sql)).or_else(db_error);
            run("BEGIN")?;
            match interp.call_block(&body, Vec::new()) {
                Ok(value) => run("COMMIT").map(|()| value),
                Err(ctrl) => {
                    run("ROLLBACK")?;
                    Err(ctrl)
                }
            }
        }
        _ => raise("NoMethodError", format!("unknown method `{name}` for a database")),
    }
}

pub(crate) fn db_error<'p, T>(message: String) -> Result<T, Ctrl<'p>> {
    raise("DbError", message)
}

/// The parameters (`[…]`, second argument); untrusted ones only in reads.
fn params<'p>(args: &Args<'p>, write: bool) -> Result<Vec<Cell>, Ctrl<'p>> {
    let Some(list) = args.pos.get(1) else { return Ok(Vec::new()) };
    if write && list.contains_taint() {
        return raise("TaintError", "an untrusted value reaches a database write (effect `db.write`) without validation");
    }
    let Value::Array(items) = list.untainted() else {
        return raise("TypeError", format!("parameters are an array, got {}", list.inspect()));
    };
    items.borrow().iter().map(cell).collect()
}

pub(crate) fn cell<'p>(value: &Value<'p>) -> Result<Cell, Ctrl<'p>> {
    Ok(match value.untainted() {
        Value::Nil => Cell::Null,
        Value::Bool(b) => Cell::Bool(*b),
        Value::Int(n) => Cell::Int(*n),
        Value::Float(f) | Value::Money(f) => Cell::Float(*f),
        Value::Str(s) | Value::Symbol(s) => Cell::Text(s.to_string()),
        other => return raise("TypeError", format!("a database parameter cannot be {}", other.inspect())),
    })
}

pub(crate) fn cell_value<'p>(cell: Cell) -> Value<'p> {
    match cell {
        Cell::Null => Value::Nil,
        Cell::Bool(b) => Value::Bool(b),
        Cell::Int(n) => Value::Int(n),
        Cell::Float(f) => Value::Float(f),
        Cell::Text(s) => Value::str(s),
    }
}

/// A row as a hash (column → value), or as a record of type `record`.
fn row_value<'p>(interp: &mut Interp<'p>, row: grenat_db::Row, record: Option<&str>) -> R<'p> {
    match record {
        None => Ok(Value::Hash(Arc::new(Mutex::new(row.into_iter().map(|(k, c)| (Value::str(k), cell_value(c))).collect())))),
        Some(ty) => {
            let named = row.into_iter().map(|(k, c)| (k, cell_value(c))).collect();
            interp.construct(ty, Args { named, ..Args::default() })
        }
    }
}
