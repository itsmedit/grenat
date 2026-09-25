//! PostgreSQL, through its wire protocol.

use postgres::types::{ToSql, Type};

use crate::placeholders::numbered;
use crate::{Cell, Connection, Row};

pub(crate) struct Postgres(postgres::Client);

impl Postgres {
    pub(crate) fn connect(url: &str) -> Result<Postgres, String> {
        postgres::Client::connect(url, postgres::NoTls).map(Postgres).map_err(|e| format!("cannot connect: {}", message(e)))
    }
}

/// The database's own message (the error's `Display` is only "db error").
fn message(e: postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => db.message().to_string(),
        None => e.to_string(),
    }
}

/// Each cell as the type PostgreSQL expects for its parameter.
fn params(cells: &[Cell], types: &[Type]) -> Result<Vec<Box<dyn ToSql + Sync>>, String> {
    cells.iter().zip(types).enumerate().map(|(i, (cell, ty))| param(cell, ty).ok_or_else(|| {
        format!("parameter {} ({cell:?}) cannot be a `{ty}`", i + 1)
    })).collect()
}

fn param(cell: &Cell, ty: &Type) -> Option<Box<dyn ToSql + Sync>> {
    Some(match (cell, ty) {
        (Cell::Null, &Type::BOOL) => Box::new(Option::<bool>::None),
        (Cell::Null, &Type::INT2) => Box::new(Option::<i16>::None),
        (Cell::Null, &Type::INT4) => Box::new(Option::<i32>::None),
        (Cell::Null, &Type::INT8) => Box::new(Option::<i64>::None),
        (Cell::Null, &Type::FLOAT4) => Box::new(Option::<f32>::None),
        (Cell::Null, &Type::FLOAT8) => Box::new(Option::<f64>::None),
        (Cell::Null, _) => Box::new(Option::<String>::None),
        (Cell::Bool(b), &Type::BOOL) => Box::new(*b),
        (Cell::Int(n), &Type::INT2) => Box::new(i16::try_from(*n).ok()?),
        (Cell::Int(n), &Type::INT4) => Box::new(i32::try_from(*n).ok()?),
        (Cell::Int(n), &Type::INT8) => Box::new(*n),
        (Cell::Int(n), &Type::FLOAT8) => Box::new(*n as f64),
        (Cell::Int(n), &Type::FLOAT4) => Box::new(*n as f32),
        (Cell::Float(f), &Type::FLOAT8) => Box::new(*f),
        (Cell::Float(f), &Type::FLOAT4) => Box::new(*f as f32),
        (Cell::Text(s), &Type::TEXT | &Type::VARCHAR | &Type::BPCHAR | &Type::NAME) => Box::new(s.clone()),
        _ => return None,
    })
}

fn cell(row: &postgres::Row, i: usize) -> Result<Cell, String> {
    let column = &row.columns()[i];
    let get = |e: postgres::Error| format!("column `{}`: {e}", column.name());
    Ok(match *column.type_() {
        Type::BOOL => row.try_get::<_, Option<bool>>(i).map_err(get)?.map_or(Cell::Null, Cell::Bool),
        Type::INT2 => row.try_get::<_, Option<i16>>(i).map_err(get)?.map_or(Cell::Null, |n| Cell::Int(n.into())),
        Type::INT4 => row.try_get::<_, Option<i32>>(i).map_err(get)?.map_or(Cell::Null, |n| Cell::Int(n.into())),
        Type::INT8 => row.try_get::<_, Option<i64>>(i).map_err(get)?.map_or(Cell::Null, Cell::Int),
        Type::FLOAT4 => row.try_get::<_, Option<f32>>(i).map_err(get)?.map_or(Cell::Null, |f| Cell::Float(f.into())),
        Type::FLOAT8 => row.try_get::<_, Option<f64>>(i).map_err(get)?.map_or(Cell::Null, Cell::Float),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => {
            row.try_get::<_, Option<String>>(i).map_err(get)?.map_or(Cell::Null, Cell::Text)
        }
        ref other => {
            return Err(format!(
                "column `{}` has type `{other}`: cast it in the query (`{}::text`, `::float8`…)",
                column.name(),
                column.name()
            ));
        }
    })
}

impl Connection for Postgres {
    fn query(&mut self, sql: &str, cells: &[Cell]) -> Result<Vec<Row>, String> {
        let statement = self.0.prepare(&numbered(sql)).map_err(message)?;
        let owned = params(cells, statement.params())?;
        let refs: Vec<&(dyn ToSql + Sync)> = owned.iter().map(|p| p.as_ref()).collect();
        let rows = self.0.query(&statement, &refs).map_err(message)?;
        rows.iter()
            .map(|row| {
                (0..row.len()).map(|i| Ok((row.columns()[i].name().to_string(), cell(row, i)?))).collect()
            })
            .collect()
    }

    fn execute(&mut self, sql: &str, cells: &[Cell]) -> Result<u64, String> {
        let statement = self.0.prepare(&numbered(sql)).map_err(message)?;
        let owned = params(cells, statement.params())?;
        let refs: Vec<&(dyn ToSql + Sync)> = owned.iter().map(|p| p.as_ref()).collect();
        self.0.execute(&statement, &refs).map_err(message)
    }

    fn batch(&mut self, sql: &str) -> Result<(), String> {
        self.0.batch_execute(sql).map_err(message)
    }
}
