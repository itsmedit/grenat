//! Vector fields of records (`embedding: Vector(1536)`) and the search of
//! the nearest ones:
//!
//! ```ruby
//! struct Doc
//!   table :docs
//!   id: Int?
//!   text: String
//!   embedding: Vector(1536)
//! end
//!
//! migration "001_docs" do |db|
//!   db.migrate("CREATE TABLE docs (id #{db.primary_key}, text TEXT NOT NULL, embedding #{db.vector(1536)} NOT NULL)")
//! end
//!
//! Doc.nearest(:embedding, embed(:docs, question), limit: 5, where: {lang: "fr"}, max_distance: 0.6)
//! ```
//!
//! `nearest` ranks by cosine distance, nearest first. Where PostgreSQL has
//! pgvector, the column is a `vector(n)` and the database ranks (`<=>`, an
//! index if one is made); elsewhere vectors are bytes and every vector the
//! filters keep is read and compared here (see `grenat_db::vectors`).

use grenat_ast::Type;
use grenat_db::Cell;
use grenat_db::vectors::{self, Storage};

use crate::builtins::{db_error, number};
use crate::eval::embeddings::floats;
use crate::eval::records::quoted;
use crate::prelude::*;

/// How many records `nearest` gives without `limit:`.
const DEFAULT_LIMIT: usize = 5;

/// The size of a `Vector(n)` type (through `?` and `~`).
pub(crate) fn vector_size(ty: &Type) -> Option<usize> {
    match ty {
        Type::Optional(inner, _) | Type::Tainted(inner, _) => vector_size(inner),
        Type::Named { path, args, .. } if path.last().is_some_and(|n| n.name == "Vector") => match args.as_slice() {
            [Type::Size(n, _)] => Some(*n as usize),
            _ => None,
        },
        _ => None,
    }
}

/// A vector read from a column: an array of floats, or `nil`.
pub(crate) fn vector_value<'p>(cell: &Cell) -> R<'p> {
    match vectors::from_cell(cell) {
        Ok(Some(v)) => Ok(Value::array(v.into_iter().map(Value::Float).collect())),
        Ok(None) => Ok(Value::Nil),
        Err(e) => db_error(format!("not a vector: {e}")),
    }
}

impl<'p> Interp<'p> {
    /// The size of the vector field `column` of the record `ty`, if it is one.
    pub(crate) fn vector_field(&self, ty: &str, column: &str) -> Option<usize> {
        let field = self.types.get(ty)?.fields.iter().find(|f| f.name.name == column)?;
        vector_size(field.ty.as_ref()?)
    }

    /// How `table.column` holds its vectors.
    fn storage(&self, connection: &crate::SharedConnection, table: &str, column: &str) -> Result<Storage, Ctrl<'p>> {
        grenat_green::blocking(|| vectors::storage(&mut **connection.lock(), table, column)).or_else(db_error)
    }

    /// `column` in a `SELECT`: pgvector's vectors are read as their text.
    pub(crate) fn selected(
        &self,
        connection: &crate::SharedConnection,
        ty: &str,
        table: &str,
        column: &str,
    ) -> Result<String, Ctrl<'p>> {
        let name = quoted(column);
        if self.vector_field(ty, column).is_some() && self.storage(connection, table, column)? == Storage::PgVector {
            return Ok(format!("{name}::text AS {name}"));
        }
        Ok(name)
    }

    /// A vector to write in `table.column`: its cell, and its placeholder
    /// (pgvector's is cast from text).
    pub(crate) fn vector_param(
        &self,
        connection: &crate::SharedConnection,
        (ty, table, column): (&str, &str, &str),
        size: usize,
        value: &Value<'p>,
    ) -> Result<(Cell, &'static str), Ctrl<'p>> {
        if matches!(value.untainted(), Value::Nil) {
            return Ok((Cell::Null, "?"));
        }
        let vector = floats(value)?;
        if vector.len() != size {
            return raise(
                "TypeError",
                format!("`{ty}.{column}` is a `Vector({size})`: a vector of {} floats cannot be stored", vector.len()),
            );
        }
        Ok(match self.storage(connection, table, column)? {
            Storage::PgVector => (Cell::Text(vectors::pgvector_text(&vector)), "?::text::vector"),
            Storage::Bytes => (Cell::Blob(vectors::encode(&vector)), "?"),
        })
    }

    /// `Doc.nearest(:embedding, vector, limit: 5, where: {…}, max_distance: 0.5)`.
    pub(crate) fn nearest(&mut self, ty: &str, table: &str, args: &Args<'p>) -> R<'p> {
        self.check_effect("db.read")?;
        let column = match args.pos.first().map(Value::untainted) {
            Some(Value::Symbol(s)) => s.to_string(),
            _ => {
                return raise(
                    "ArgumentError",
                    format!("`{ty}.nearest` expects a vector field: `nearest(:embedding, vector)`"),
                );
            }
        };
        let Some(size) = self.vector_field(ty, &column) else {
            return raise("ArgumentError", format!("`{ty}.{column}` is not a vector field (`{column}: Vector(1536)`)"));
        };
        let query = match args.pos.get(1) {
            Some(v) => floats(v)?,
            None => return raise("ArgumentError", format!("`{ty}.nearest` expects a vector to compare with")),
        };
        if query.len() != size {
            return raise(
                "TypeError",
                format!("`{ty}.{column}` is a `Vector({size})`: a vector of {} floats cannot be compared", query.len()),
            );
        }
        let (mut limit, mut filters, mut max_distance) = (DEFAULT_LIMIT, Vec::new(), None);
        for (option, value) in &args.named {
            match (option.as_str(), value.untainted()) {
                ("limit", Value::Int(n)) if *n > 0 => limit = *n as usize,
                ("where", Value::Hash(pairs)) => {
                    filters = pairs.borrow().iter().map(|(k, v)| (k.to_display(), v.clone())).collect();
                }
                ("max_distance", v) if number(v).is_some() => max_distance = number(v),
                (option, v) => {
                    return raise("ArgumentError", format!("invalid `nearest` option `{option}: {}`", v.inspect()));
                }
            }
        }
        let connection = self.app_connection()?;
        let columns = self.columns(ty);
        let mut list = Vec::with_capacity(columns.len());
        for c in &columns {
            list.push(self.selected(&connection, ty, table, c)?);
        }
        let (condition, mut params) = self.conditions(ty, &filters)?;
        let column_q = quoted(&column);
        let not_null = format!("{} {column_q} IS NOT NULL", if condition.is_empty() { " WHERE" } else { " AND" });
        let from = format!("SELECT {} FROM {}{condition}{not_null}", list.join(", "), quoted(table));
        if self.storage(&connection, table, &column)? == Storage::PgVector {
            let text = vectors::pgvector_text(&query);
            let mut sql = from;
            if let Some(max) = max_distance {
                sql.push_str(&format!(" AND ({column_q} <=> ?::text::vector) <= ?"));
                params.extend([Cell::Text(text.clone()), Cell::Float(max)]);
            }
            sql.push_str(&format!(" ORDER BY {column_q} <=> ?::text::vector LIMIT {limit}"));
            params.push(Cell::Text(text));
            let rows = grenat_green::blocking(|| connection.lock().query(&sql, &params)).or_else(db_error)?;
            let mut records = Vec::with_capacity(rows.len());
            for row in rows {
                records.push(self.record_of(ty, row)?);
            }
            return Ok(Value::array(records));
        }
        // brute force: every vector the filters keep, compared here
        let order = if columns.iter().any(|c| c == "id") { " ORDER BY \"id\"" } else { "" };
        let sql = format!("{from}{order}");
        let rows = grenat_green::blocking(|| connection.lock().query(&sql, &params)).or_else(db_error)?;
        let mut candidates = Vec::with_capacity(rows.len());
        for row in rows {
            let stored = row.iter().find(|(c, _)| *c == column).map(|(_, cell)| vectors::from_cell(cell));
            match stored {
                Some(Ok(Some(vector))) if vector.len() == size => candidates.push((row, vector)),
                Some(Ok(Some(vector))) => {
                    return raise(
                        "DbError",
                        format!("a stored `{ty}.{column}` has {} dimensions, not {size}", vector.len()),
                    );
                }
                Some(Err(e)) => return raise("DbError", format!("`{ty}.{column}`: {e}")),
                _ => {}
            }
        }
        let mut records = Vec::new();
        for (row, _) in vectors::nearest(candidates, &query, limit, max_distance) {
            records.push(self.record_of(ty, row)?);
        }
        Ok(Value::array(records))
    }
}
