//! Events (`grenat_events`): what went wrong while serving — a request, a
//! webhook, a schedule or a job that failed — and among them the refusals:
//! untrusted data stopped at a sink, a capability not granted, a human who
//! said no, a budget spent.

use grenat_db::{Cell, Connection};

use crate::Result;
use crate::row::{float, int, text};

pub const TABLE: &str = "grenat_events";

/// Error types that are refusals: the program stopped itself.
pub const REFUSALS: [&str; 4] = ["TaintError", "CapabilityError", "ApprovalDenied", "BudgetExceeded"];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Event {
    pub id: i64,
    pub at: f64,
    /// `request`, `webhook`, `schedule` or `job`.
    pub source: String,
    /// Which one: `POST /tickets`, `job 12 (triage)`…
    pub subject: String,
    /// The error's type (`TaintError`…).
    pub error: String,
    pub message: String,
}

impl Event {
    pub fn is_refusal(&self) -> bool {
        REFUSALS.contains(&self.error.as_str())
    }
}

pub fn ensure(db: &mut dyn Connection) -> Result<()> {
    let key = db.dialect().primary_key();
    db.batch(&format!(
        "CREATE TABLE IF NOT EXISTS {TABLE} (id {key}, at FLOAT NOT NULL, source TEXT NOT NULL, \
         subject TEXT NOT NULL, error TEXT NOT NULL, message TEXT NOT NULL)"
    ))
}

pub fn record(db: &mut dyn Connection, at: f64, source: &str, subject: &str, error: &str, message: &str) -> Result<()> {
    ensure(db)?;
    let sql = format!("INSERT INTO {TABLE} (at, source, subject, error, message) VALUES (?, ?, ?, ?, ?)");
    let params = [Cell::Float(at), Cell::Text(source.into()), Cell::Text(subject.into()), Cell::Text(error.into()), Cell::Text(message.into())];
    db.execute(&sql, &params).map(drop)
}

/// The latest events, newest first: all, or only refusals.
pub fn latest(db: &mut dyn Connection, refusals_only: bool, limit: usize) -> Result<Vec<Event>> {
    ensure(db)?;
    let limit = Cell::Int(limit.min(10_000) as i64);
    let columns = "id, at, source, subject, error, message";
    let rows = if refusals_only {
        let marks = vec!["?"; REFUSALS.len()].join(", ");
        let mut params: Vec<Cell> = REFUSALS.iter().map(|r| Cell::Text(r.to_string())).collect();
        params.push(limit);
        db.query(&format!("SELECT {columns} FROM {TABLE} WHERE error IN ({marks}) ORDER BY id DESC LIMIT ?"), &params)?
    } else {
        db.query(&format!("SELECT {columns} FROM {TABLE} ORDER BY id DESC LIMIT ?"), &[limit])?
    };
    Ok(rows
        .iter()
        .map(|r| Event { id: int(r, 0), at: float(r, 1), source: text(r, 2), subject: text(r, 3), error: text(r, 4), message: text(r, 5) })
        .collect())
}
