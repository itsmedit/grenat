//! Vectors (embeddings) in a database, searched by cosine distance.
//!
//! A vector column is `vector(n)` where PostgreSQL has the pgvector
//! extension — the database searches it (`<=>`) — and bytes elsewhere
//! (`BLOB` on SQLite, `BYTEA` on PostgreSQL without pgvector): 32-bit
//! floats, little-endian, the precision pgvector keeps too. Bytes are
//! searched here, by brute force ([`nearest`]): every vector of the table
//! is read and compared, which is fine up to tens of thousands of rows
//! (1536 dimensions are 6 KB a row: 10,000 rows are 60 MB read per search).

use crate::{Cell, Connection, Dialect};

/// How a vector column holds its floats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// 32-bit floats as bytes, compared in Rust.
    Bytes,
    /// pgvector's `vector(n)`, compared by PostgreSQL.
    PgVector,
}

/// A vector as bytes: 32-bit floats, little-endian.
pub fn encode(vector: &[f64]) -> Vec<u8> {
    vector.iter().flat_map(|x| (*x as f32).to_le_bytes()).collect()
}

/// The vector `bytes` hold.
pub fn decode(bytes: &[u8]) -> Result<Vec<f64>, String> {
    if !bytes.len().is_multiple_of(4) {
        return Err(format!("{} bytes are not a vector of 32-bit floats", bytes.len()));
    }
    Ok(bytes.as_chunks::<4>().0.iter().map(|c| f64::from(f32::from_le_bytes(*c))).collect())
}

/// A vector as pgvector reads it: `[0.1,0.2,0.3]`.
pub fn pgvector_text(vector: &[f64]) -> String {
    let items: Vec<String> = vector.iter().map(|x| (*x as f32).to_string()).collect();
    format!("[{}]", items.join(","))
}

/// The vector of pgvector's text (`[0.1,0.2,0.3]`).
pub fn parse_pgvector(text: &str) -> Result<Vec<f64>, String> {
    let inner = text
        .trim()
        .strip_prefix('[')
        .and_then(|t| t.strip_suffix(']'))
        .ok_or_else(|| format!("`{text}` is not a pgvector value"))?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    inner.split(',').map(|x| x.trim().parse::<f64>().map_err(|_| format!("`{x}` is not a number"))).collect()
}

/// The vector a cell holds: bytes, or pgvector's text.
pub fn from_cell(cell: &Cell) -> Result<Option<Vec<f64>>, String> {
    match cell {
        Cell::Null => Ok(None),
        Cell::Blob(bytes) => decode(bytes).map(Some),
        Cell::Text(text) => parse_pgvector(text).map(Some),
        other => Err(format!("{other:?} is not a vector")),
    }
}

/// `1 - cos(a, b)`: 0 for the same direction, 2 for opposite ones. A zero
/// vector has no direction: it is at distance 1 from everything, as an
/// orthogonal one. Each vector is first divided by its largest component,
/// so that only directions count: no square overflows for a huge vector or
/// vanishes for a tiny one.
pub fn cosine_distance(a: &[f64], b: &[f64]) -> f64 {
    let largest = |v: &[f64]| v.iter().fold(0.0_f64, |m, x| m.max(x.abs()));
    let (scale_a, scale_b) = (largest(a), largest(b));
    if scale_a == 0.0 || scale_b == 0.0 || !scale_a.is_finite() || !scale_b.is_finite() {
        return 1.0;
    }
    let (mut dot, mut norm_a, mut norm_b) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x / scale_a, y / scale_b);
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    1.0 - dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// The `limit` candidates nearest to `query`, nearest first, with their
/// distance; those farther than `max_distance` are left out. Ties keep the
/// candidates' order.
pub fn nearest<T>(
    candidates: Vec<(T, Vec<f64>)>,
    query: &[f64],
    limit: usize,
    max_distance: Option<f64>,
) -> Vec<(T, f64)> {
    let mut scored: Vec<(T, f64)> = candidates
        .into_iter()
        .map(|(item, vector)| {
            let distance = cosine_distance(&vector, query);
            (item, distance)
        })
        .filter(|(_, d)| max_distance.is_none_or(|max| *d <= max))
        .collect();
    scored.sort_by(|a, b| a.1.total_cmp(&b.1));
    scored.truncate(limit);
    scored
}

/// The column type of a vector of `size` floats, for a migration:
/// `vector(size)` where PostgreSQL has pgvector (enabled here when the
/// server offers it), bytes otherwise. The note says why pgvector could not
/// be used although the server has it.
pub fn column_type(db: &mut dyn Connection, size: u32) -> Result<(String, Option<String>), String> {
    if db.dialect() == Dialect::Sqlite {
        return Ok(("BLOB".into(), None));
    }
    if has_type(db)? {
        return Ok((format!("vector({size})"), None));
    }
    let available = db.query("SELECT 1 AS found FROM pg_available_extensions WHERE name = 'vector'", &[])?;
    if available.is_empty() {
        return Ok(("BYTEA".into(), None));
    }
    // in a savepoint: a refusal (privileges) must not end the migration's transaction
    let in_transaction = db.batch("SAVEPOINT grenat_vectors").is_ok();
    match db.batch("CREATE EXTENSION IF NOT EXISTS vector") {
        Ok(()) => {
            if in_transaction {
                db.batch("RELEASE SAVEPOINT grenat_vectors")?;
            }
            Ok((format!("vector({size})"), None))
        }
        Err(e) => {
            if in_transaction {
                db.batch("ROLLBACK TO SAVEPOINT grenat_vectors; RELEASE SAVEPOINT grenat_vectors")?;
            }
            let note = format!(
                "the server has pgvector, but it could not be enabled ({e}): vectors are stored as bytes and searched by brute force; enable it with `CREATE EXTENSION vector` as an administrator"
            );
            Ok(("BYTEA".into(), Some(note)))
        }
    }
}

/// Whether the database has pgvector's `vector` type (its extension is enabled).
fn has_type(db: &mut dyn Connection) -> Result<bool, String> {
    Ok(!db.query("SELECT 1 AS found FROM pg_type WHERE typname = 'vector'", &[])?.is_empty())
}

/// How `table.column` holds its vectors.
pub fn storage(db: &mut dyn Connection, table: &str, column: &str) -> Result<Storage, String> {
    if db.dialect() == Dialect::Sqlite {
        return Ok(Storage::Bytes);
    }
    let sql = "SELECT format_type(a.atttypid, a.atttypmod) AS type FROM pg_attribute a \
               WHERE a.attrelid = to_regclass(?) AND a.attname = ? AND NOT a.attisdropped";
    let quoted = format!("\"{}\"", table.replace('"', "\"\""));
    let rows = db.query(sql, &[Cell::Text(quoted), Cell::Text(column.to_string())])?;
    match rows.first().and_then(|r| r.first()).map(|(_, c)| c) {
        Some(Cell::Text(ty)) if ty.starts_with("vector") => Ok(Storage::PgVector),
        Some(Cell::Text(ty)) if ty == "bytea" => Ok(Storage::Bytes),
        Some(Cell::Text(ty)) => Err(format!(
            "the column `{column}` of `{table}` is a `{ty}`: a vector column is a `vector(n)` or a `bytea` (`db.vector(n)` in a migration)"
        )),
        _ => Err(format!("no column `{column}` in the table `{table}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_round_trip_as_bytes_and_as_text() {
        let v = vec![0.5, -1.0, 0.25, 3.0];
        assert_eq!(encode(&v).len(), 16);
        assert_eq!(decode(&encode(&v)).unwrap(), v);
        assert!(decode(&[1, 2, 3]).unwrap_err().contains("3 bytes"));
        assert_eq!(pgvector_text(&v), "[0.5,-1,0.25,3]");
        assert_eq!(parse_pgvector("[0.5,-1,0.25,3]").unwrap(), v);
        assert_eq!(parse_pgvector("[]").unwrap(), Vec::<f64>::new());
        assert!(parse_pgvector("0.5").is_err());
        assert!(parse_pgvector("[a]").is_err());
        assert_eq!(from_cell(&Cell::Null).unwrap(), None);
        assert_eq!(from_cell(&Cell::Blob(encode(&v))).unwrap(), Some(v.clone()));
        assert_eq!(from_cell(&Cell::Text("[1,2]".into())).unwrap(), Some(vec![1.0, 2.0]));
        assert!(from_cell(&Cell::Int(1)).is_err());
    }

    #[test]
    fn cosine_distance_ranks_by_direction() {
        assert!(cosine_distance(&[1.0, 0.0], &[2.0, 0.0]).abs() < 1e-12);
        assert!((cosine_distance(&[1.0, 0.0], &[0.0, 1.0]) - 1.0).abs() < 1e-12);
        assert!((cosine_distance(&[1.0, 0.0], &[-1.0, 0.0]) - 2.0).abs() < 1e-12);
        assert_eq!(cosine_distance(&[0.0, 0.0], &[1.0, 0.0]), 1.0);
        // only the direction counts, whatever the magnitude
        assert!(cosine_distance(&[1.0e200, 0.0], &[1.0, 0.0]).abs() < 1e-12);
        assert!((cosine_distance(&[1.0e200, 0.0], &[0.0, 1.0]) - 1.0).abs() < 1e-12);
        assert!(cosine_distance(&[1.0e-200, 0.0], &[1.0, 0.0]).abs() < 1e-12);
        assert!((cosine_distance(&[1.0e-200, 1.0e-200], &[1.0e200, 0.0]) - (1.0 - 0.5_f64.sqrt())).abs() < 1e-12);
        let candidates = vec![("east", vec![1.0, 0.0]), ("north", vec![0.0, 1.0]), ("northeast", vec![1.0, 1.0])];
        let found = nearest(candidates.clone(), &[1.0, 0.1], 2, None);
        assert_eq!(found.iter().map(|(n, _)| *n).collect::<Vec<_>>(), ["east", "northeast"]);
        let close = nearest(candidates, &[1.0, 0.1], 5, Some(0.1));
        assert_eq!(close.iter().map(|(n, _)| *n).collect::<Vec<_>>(), ["east"]);
    }
}
