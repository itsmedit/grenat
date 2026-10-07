//! Handled files (`grenat_files`): for each `on_file` watch (its pattern)
//! and each file, the content (a digest) handed over last — so that a file
//! handled before `grenat serve` restarts is not handed over again. A file
//! gone from its folder is forgotten.

use grenat_db::{Cell, Connection};

use crate::Result;
use crate::row::text;

pub const TABLE: &str = "grenat_files";

pub fn ensure(db: &mut dyn Connection) -> Result<()> {
    let key = db.dialect().primary_key();
    db.batch(&format!(
        "CREATE TABLE IF NOT EXISTS {TABLE} (id {key}, watch TEXT NOT NULL, path TEXT NOT NULL, \
         digest TEXT NOT NULL, at FLOAT NOT NULL)"
    ))
}

/// The files of `watch` handled so far: (path, digest).
pub fn handled(db: &mut dyn Connection, watch: &str) -> Result<Vec<(String, String)>> {
    ensure(db)?;
    let rows = db.query(
        &format!("SELECT path, digest FROM {TABLE} WHERE watch = ? ORDER BY path"),
        &[Cell::Text(watch.into())],
    )?;
    Ok(rows.iter().map(|r| (text(r, 0), text(r, 1))).collect())
}

/// `path`'s content `digest` was handed over (or given up) at `at`.
pub fn record(db: &mut dyn Connection, watch: &str, path: &str, digest: &str, at: f64) -> Result<()> {
    forget(db, watch, path)?;
    let sql = format!("INSERT INTO {TABLE} (watch, path, digest, at) VALUES (?, ?, ?, ?)");
    let params = [Cell::Text(watch.into()), Cell::Text(path.into()), Cell::Text(digest.into()), Cell::Float(at)];
    db.execute(&sql, &params).map(drop)
}

/// `path` left the folder of `watch`.
pub fn forget(db: &mut dyn Connection, watch: &str, path: &str) -> Result<()> {
    ensure(db)?;
    let sql = format!("DELETE FROM {TABLE} WHERE watch = ? AND path = ?");
    db.execute(&sql, &[Cell::Text(watch.into()), Cell::Text(path.into())]).map(drop)
}
