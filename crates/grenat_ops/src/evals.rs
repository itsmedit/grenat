//! Eval runs (`grenat_evals`): each `grenat eval` of each eval, kept to see
//! quality over time.

use grenat_db::{Cell, Connection};

use crate::Result;
use crate::row::{boolean, float, int, text};

pub const TABLE: &str = "grenat_evals";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Run {
    pub id: i64,
    pub at: f64,
    pub name: String,
    /// The mean score, from 0 to 1.
    pub score: f64,
    pub threshold: f64,
    pub passed: bool,
    pub rows: i64,
    /// Rows that raised an error (scored 0).
    pub failed_rows: i64,
    pub cost_usd: f64,
    pub seconds: f64,
}

pub fn ensure(db: &mut dyn Connection) -> Result<()> {
    let key = db.dialect().primary_key();
    db.batch(&format!(
        "CREATE TABLE IF NOT EXISTS {TABLE} (id {key}, at FLOAT NOT NULL, name TEXT NOT NULL, score FLOAT NOT NULL, \
         threshold FLOAT NOT NULL, passed BOOLEAN NOT NULL, rows_count INTEGER NOT NULL, failed_rows INTEGER NOT NULL, \
         cost_usd FLOAT NOT NULL, seconds FLOAT NOT NULL)"
    ))
}

pub fn record(db: &mut dyn Connection, run: &Run) -> Result<()> {
    ensure(db)?;
    let sql = format!(
        "INSERT INTO {TABLE} (at, name, score, threshold, passed, rows_count, failed_rows, cost_usd, seconds) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
    );
    let params = [
        Cell::Float(run.at),
        Cell::Text(run.name.clone()),
        Cell::Float(run.score),
        Cell::Float(run.threshold),
        Cell::Bool(run.passed),
        Cell::Int(run.rows),
        Cell::Int(run.failed_rows),
        Cell::Float(run.cost_usd),
        Cell::Float(run.seconds),
    ];
    db.execute(&sql, &params).map(drop)
}

/// Every run, oldest first.
pub fn history(db: &mut dyn Connection) -> Result<Vec<Run>> {
    ensure(db)?;
    let sql = format!(
        "SELECT id, at, name, score, threshold, passed, rows_count, failed_rows, cost_usd, seconds FROM {TABLE} ORDER BY at, id"
    );
    Ok(db
        .query(&sql, &[])?
        .iter()
        .map(|r| Run {
            id: int(r, 0),
            at: float(r, 1),
            name: text(r, 2),
            score: float(r, 3),
            threshold: float(r, 4),
            passed: boolean(r, 5),
            rows: int(r, 6),
            failed_rows: int(r, 7),
            cost_usd: float(r, 8),
            seconds: float(r, 9),
        })
        .collect())
}

/// `runs` by eval name (in order of first run), each oldest first.
pub fn by_name(runs: Vec<Run>) -> Vec<(String, Vec<Run>)> {
    let mut groups: Vec<(String, Vec<Run>)> = Vec::new();
    for run in runs {
        match groups.iter_mut().find(|(name, _)| *name == run.name) {
            Some((_, group)) => group.push(run),
            None => groups.push((run.name.clone(), vec![run])),
        }
    }
    groups
}
