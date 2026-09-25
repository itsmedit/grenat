//! Records: structs stored in the application's database.
//!
//! ```ruby
//! database Env.fetch("DATABASE_URL")
//!
//! struct Ticket
//!   table :tickets
//!   id: Int?
//!   subject: String
//!   status: String = "open"
//! end
//!
//! migration "001_create_tickets" do |db|
//!   db.migrate("CREATE TABLE tickets (id INTEGER PRIMARY KEY, subject TEXT NOT NULL, status TEXT NOT NULL)")
//! end
//!
//! t = Ticket.create(subject: "Bug")   # Ticket.find(1), Ticket.where(status: "open"), Ticket.all, Ticket.count
//! t.with(status: "closed").save
//! t.delete
//! ```
//!
//! Columns are the struct's fields; `id` is the primary key, given by the
//! database. Reads are `db.read`, writes `db.write`; an untrusted value is
//! never written unchecked. `grenat migrate` applies the migrations not
//! applied yet; in `grenat test`, each test gets a new in-memory SQLite
//! database, migrated.

use grenat_ast::{Arg, ExprKind, Type};
use grenat_db::Cell;

use crate::builtins::{cell, cell_value, db_error};
use crate::prelude::*;

/// Where migrations applied are recorded.
const MIGRATIONS_TABLE: &str = "grenat_migrations";

pub(crate) struct Migration<'p> {
    pub name: String,
    pub block: Value<'p>,
}

/// `"name"`: an SQL identifier (names come from declarations, quoted anyway).
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

impl<'p> Interp<'p> {
    /// The table of a struct (`table :tickets`), if it is a record.
    pub(crate) fn table_of(&self, ty: &str) -> Option<String> {
        let info = self.types.get(ty)?;
        info.directives.iter().find(|d| d.name.name == "table").and_then(|d| match d.args.first() {
            Some(Arg::Pos(e)) => match &e.kind {
                ExprKind::Symbol(s) => Some(s.clone()),
                ExprKind::Str(_) => crate::builtins::literal_text(e),
                _ => None,
            },
            _ => None,
        })
    }

    /// `database "url"`: the application's database (a test's own in tests).
    pub(crate) fn declare_database(&mut self, args: &Args<'p>) -> R<'p> {
        if self.offline {
            return Ok(Value::Nil);
        }
        let database = crate::builtins::connect(self, args)?;
        *self.app_db.borrow_mut() = Some(database);
        Ok(Value::Nil)
    }

    /// `migration "001_name" do |db| … end`.
    pub(crate) fn declare_migration(&mut self, args: &Args<'p>) -> R<'p> {
        let name = crate::builtins::str_arg(args, 0, "migration")?.to_string();
        let block = crate::builtins::block(args, "migration")?;
        if self.migrations.borrow().iter().any(|m| m.name == name) {
            return raise("ArgumentError", format!("migration `{name}` is declared twice"));
        }
        self.migrations.borrow_mut().push(Migration { name, block });
        Ok(Value::Nil)
    }

    /// A new in-memory database with every migration applied (a test's).
    pub(crate) fn fresh_test_database(&mut self) -> Result<(), Ctrl<'p>> {
        let connection = grenat_db::connect("sqlite::memory:").or_else(db_error)?;
        let database = self.register_database(connection);
        *self.app_db.borrow_mut() = Some(database.clone());
        let migrations: Vec<Value<'p>> = self.migrations.borrow().iter().map(|m| m.block.clone()).collect();
        for block in migrations {
            self.call_block(&block, vec![database.clone()])?;
        }
        Ok(())
    }

    /// Applies the migrations not applied yet, in order: their names.
    pub(crate) fn apply_migrations(&mut self) -> Result<Vec<String>, Ctrl<'p>> {
        let database = self.database()?;
        let connection = self.connection_of(&database).expect("a database");
        let setup = format!("CREATE TABLE IF NOT EXISTS {MIGRATIONS_TABLE} (name TEXT PRIMARY KEY, applied_at TEXT NOT NULL)");
        grenat_green::blocking(|| connection.lock().batch(&setup)).or_else(db_error)?;
        let done = grenat_green::blocking(|| connection.lock().query(&format!("SELECT name FROM {MIGRATIONS_TABLE}"), &[]))
            .or_else(db_error)?;
        let done: Vec<String> = done.into_iter().filter_map(|row| match row.into_iter().next() {
            Some((_, Cell::Text(name))) => Some(name),
            _ => None,
        }).collect();
        let pending: Vec<(String, Value<'p>)> = self
            .migrations
            .borrow()
            .iter()
            .filter(|m| !done.contains(&m.name))
            .map(|m| (m.name.clone(), m.block.clone()))
            .collect();
        let mut applied = Vec::new();
        for (name, block) in pending {
            let run = |sql: &str| grenat_green::blocking(|| connection.lock().batch(sql)).or_else(db_error);
            run("BEGIN")?;
            let result = self.call_block(&block, vec![database.clone()]).and_then(|_| {
                let record = format!("INSERT INTO {MIGRATIONS_TABLE} (name, applied_at) VALUES (?, ?)");
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                let at = grenat_serve::calendar::date(now.as_secs() as i64);
                grenat_green::blocking(|| connection.lock().execute(&record, &[Cell::Text(name.clone()), Cell::Text(at)]))
                    .or_else(db_error)
            });
            match result {
                Ok(_) => run("COMMIT")?,
                Err(ctrl) => {
                    run("ROLLBACK")?;
                    return Err(ctrl);
                }
            }
            applied.push(name);
        }
        Ok(applied)
    }

    fn database(&self) -> R<'p> {
        match self.app_db.borrow().clone() {
            Some(database) => Ok(database),
            None => raise("DbError", "no database: declare one (`database Env.fetch(\"DATABASE_URL\")`)"),
        }
    }

    fn app_connection(&self) -> Result<crate::SharedConnection, Ctrl<'p>> {
        let database = self.database()?;
        Ok(self.connection_of(&database).expect("a database"))
    }

    /// `Ticket.find/where/all/count/create`, for a struct with a table.
    pub(crate) fn record_static(&mut self, ty: &str, name: &str, args: &Args<'p>) -> Option<R<'p>> {
        let table = self.table_of(ty)?;
        Some((|| match name {
            "all" => self.select(ty, &table, &[], None),
            "where" => self.select(ty, &table, &args.named, None),
            "find" => {
                let id = crate::builtins::arg(args, 0, name)?;
                let found = self.select(ty, &table, &[("id".into(), id)], Some(1))?;
                Ok(match found {
                    Value::Array(items) => items.borrow().first().cloned().unwrap_or(Value::Nil),
                    other => other,
                })
            }
            "count" => {
                self.check_effect("db.read")?;
                let (condition, params) = conditions(&args.named)?;
                let sql = format!("SELECT count(*) AS n FROM {}{condition}", quoted(&table));
                let connection = self.app_connection()?;
                let rows = grenat_green::blocking(|| connection.lock().query(&sql, &params)).or_else(db_error)?;
                Ok(rows.first().and_then(|r| r.first()).map_or(Value::Int(0), |(_, c)| cell_value(c.clone())))
            }
            "create" => {
                let record = self.construct(ty, Args { named: args.named.clone(), ..Args::default() })?;
                self.save(ty, &table, &record)
            }
            _ => raise("NoMethodError", format!("unknown method `{ty}.{name}`: a record has all, where, find, count, create")),
        })())
    }

    /// `record.save` and `record.delete`.
    pub(crate) fn record_instance(&mut self, record: &Value<'p>, name: &str) -> Option<R<'p>> {
        let Value::Record(r) = record.untainted() else { return None };
        let ty = r.ty.to_string();
        let table = self.table_of(&ty)?;
        match name {
            "save" => Some(self.save(&ty, &table, record)),
            "delete" => Some((|| {
                self.check_effect("db.write")?;
                let id = field(&r.fields, "id").cloned().unwrap_or(Value::Nil);
                if matches!(id, Value::Nil) {
                    return raise("DbError", format!("this `{ty}` was never saved: it has no id"));
                }
                let sql = format!("DELETE FROM {} WHERE \"id\" = ?", quoted(&table));
                let connection = self.app_connection()?;
                let params = [cell(&id)?];
                grenat_green::blocking(|| connection.lock().execute(&sql, &params)).or_else(db_error)?;
                Ok(Value::Nil)
            })()),
            _ => None,
        }
    }

    fn select(&mut self, ty: &str, table: &str, conditions_of: &[(String, Value<'p>)], limit: Option<usize>) -> R<'p> {
        self.check_effect("db.read")?;
        let columns = self.columns(ty);
        let (condition, params) = conditions(conditions_of)?;
        let order = if columns.iter().any(|c| c == "id") { " ORDER BY \"id\"" } else { "" };
        let limit = limit.map_or(String::new(), |n| format!(" LIMIT {n}"));
        let list: Vec<String> = columns.iter().map(|c| quoted(c)).collect();
        let sql = format!("SELECT {} FROM {}{condition}{order}{limit}", list.join(", "), quoted(table));
        let connection = self.app_connection()?;
        let rows = grenat_green::blocking(|| connection.lock().query(&sql, &params)).or_else(db_error)?;
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            records.push(self.record_of(ty, row)?);
        }
        Ok(Value::array(records))
    }

    /// Inserts a record without an id, updates one with it; the record, with its id.
    fn save(&mut self, ty: &str, table: &str, record: &Value<'p>) -> R<'p> {
        self.check_effect("db.write")?;
        if record.contains_taint() {
            return raise("TaintError", format!("an untrusted value reaches a `{ty}` written to the database: check it first"));
        }
        let Value::Record(r) = record.untainted() else { return raise("TypeError", "not a record") };
        let id = field(&r.fields, "id").cloned().unwrap_or(Value::Nil);
        let columns: Vec<String> = self.columns(ty).into_iter().filter(|c| c != "id").collect();
        let mut params = Vec::with_capacity(columns.len() + 1);
        for column in &columns {
            params.push(cell(field(&r.fields, column).unwrap_or(&Value::Nil))?);
        }
        let has_id = r.fields.iter().any(|(k, _)| &**k == "id");
        let connection = self.app_connection()?;
        let table_q = quoted(table);
        if !matches!(id, Value::Nil) {
            let sets: Vec<String> = columns.iter().map(|c| format!("{} = ?", quoted(c))).collect();
            params.push(cell(&id)?);
            let sql = format!("UPDATE {table_q} SET {} WHERE \"id\" = ?", sets.join(", "));
            grenat_green::blocking(|| connection.lock().execute(&sql, &params)).or_else(db_error)?;
            return Ok(record.clone());
        }
        let list: Vec<String> = columns.iter().map(|c| quoted(c)).collect();
        let marks = vec!["?"; columns.len()].join(", ");
        if !has_id {
            let sql = format!("INSERT INTO {table_q} ({}) VALUES ({marks})", list.join(", "));
            grenat_green::blocking(|| connection.lock().execute(&sql, &params)).or_else(db_error)?;
            return Ok(record.clone());
        }
        let sql = format!("INSERT INTO {table_q} ({}) VALUES ({marks}) RETURNING \"id\"", list.join(", "));
        let rows = grenat_green::blocking(|| connection.lock().query(&sql, &params)).or_else(db_error)?;
        let new_id = rows.first().and_then(|r| r.first()).map_or(Value::Nil, |(_, c)| cell_value(c.clone()));
        let fields = r.fields.iter().map(|(k, v)| (k.clone(), if &**k == "id" { new_id.clone() } else { v.clone() })).collect();
        Ok(Value::record(&r.ty, fields))
    }

    fn columns(&self, ty: &str) -> Vec<String> {
        self.types[ty].fields.iter().filter(|f| !f.is_ivar).map(|f| f.name.name.clone()).collect()
    }

    /// A row as the record, cells converted by the fields' types.
    fn record_of(&mut self, ty: &str, row: grenat_db::Row) -> R<'p> {
        let fields = self.types[ty].fields.clone();
        let mut named = Vec::with_capacity(row.len());
        for (column, c) in row {
            let declared = fields.iter().find(|f| f.name.name == column).and_then(|f| f.ty.as_ref());
            let value = match (declared.map(type_name), c) {
                (Some("Bool"), Cell::Int(n)) => Value::Bool(n != 0),
                (Some("Float"), Cell::Int(n)) => Value::Float(n as f64),
                (_, c) => cell_value(c),
            };
            named.push((column, value));
        }
        self.construct(ty, Args { named, ..Args::default() })
    }
}

fn type_name(ty: &Type) -> &str {
    match ty {
        Type::Named { path, .. } => &path.last().expect("a name").name,
        Type::Optional(inner, _) | Type::Tainted(inner, _) => type_name(inner),
    }
}

/// ` WHERE "a" = ? AND "b" IS NULL`, and its parameters.
fn conditions<'p>(named: &[(String, Value<'p>)]) -> Result<(String, Vec<Cell>), Ctrl<'p>> {
    if named.is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    let mut parts = Vec::new();
    let mut params = Vec::new();
    for (column, value) in named {
        if matches!(value.untainted(), Value::Nil) {
            parts.push(format!("{} IS NULL", quoted(column)));
        } else {
            parts.push(format!("{} = ?", quoted(column)));
            params.push(cell(value)?);
        }
    }
    Ok((format!(" WHERE {}", parts.join(" AND ")), params))
}

/// `grenat migrate`: the migrations applied.
pub fn migrate(program: &grenat_ast::Program, options: crate::Options) -> Result<Vec<String>, crate::RuntimeError> {
    crate::on_interpreter_thread(|green| {
        let mut interp = Interp::new(program, options, crate::spawner(green))?;
        let result = interp.run_script().and_then(|()| interp.apply_migrations());
        result.map_err(|ctrl| interp.runtime_error(ctrl))
    })
}
