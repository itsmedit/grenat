//! Approvals that wait (`grenat_approvals`): in a job, a question to a human
//! is stored and the job waits until someone decides. A question is known
//! by its job and its rank among the questions of a run, so that the same
//! question gets the same answer when the job runs again. A decision
//! records who took it (`console`, `slack:U123`), when it is known.

use grenat_db::{Cell, Connection};

use crate::Result;
use crate::row::{float, int, opt_float, opt_text, text};
use crate::{columns, jobs};

pub const TABLE: &str = "grenat_approvals";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Pending,
    Approved,
    Denied,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Pending => "pending",
            Decision::Approved => "approved",
            Decision::Denied => "denied",
        }
    }

    fn parse(text: &str) -> Decision {
        match text {
            "approved" => Decision::Approved,
            "denied" => Decision::Denied,
            _ => Decision::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    pub id: i64,
    pub job_id: i64,
    pub rank: i64,
    pub message: String,
    pub decision: Decision,
    pub created_at: f64,
    pub decided_at: Option<f64>,
    /// Who decided (`console`, `slack:U123`); `None` when the program did.
    pub decided_by: Option<String>,
}

const COLUMNS: &str = "id, job_id, rank, message, status, created_at, decided_at, decided_by";

/// Columns added since the table's first release.
const ADDED: [(&str, &str); 1] = [("decided_by", "TEXT")];

fn approval(row: &grenat_db::Row) -> Approval {
    Approval {
        id: int(row, 0),
        job_id: int(row, 1),
        rank: int(row, 2),
        message: text(row, 3),
        decision: Decision::parse(&text(row, 4)),
        created_at: float(row, 5),
        decided_at: opt_float(row, 6),
        decided_by: opt_text(row, 7),
    }
}

pub fn ensure(db: &mut dyn Connection) -> Result<()> {
    let key = db.dialect().primary_key();
    db.batch(&format!(
        "CREATE TABLE IF NOT EXISTS {TABLE} (id {key}, job_id INTEGER NOT NULL, rank INTEGER NOT NULL, \
         message TEXT NOT NULL, status TEXT NOT NULL, created_at FLOAT NOT NULL, decided_at FLOAT, decided_by TEXT)"
    ))?;
    columns::add_missing(db, TABLE, &ADDED)
}

/// The decision on question `rank` of job `job`, if it was asked.
pub fn decision(db: &mut dyn Connection, job: i64, rank: i64) -> Result<Option<Decision>> {
    ensure(db)?;
    let rows = db.query(
        &format!("SELECT status FROM {TABLE} WHERE job_id = ? AND rank = ?"),
        &[Cell::Int(job), Cell::Int(rank)],
    )?;
    Ok(rows.first().map(|r| Decision::parse(&text(r, 0))))
}

/// Stores question `rank` of job `job`, pending: its id.
pub fn ask(db: &mut dyn Connection, job: i64, rank: i64, message: &str, now: f64) -> Result<i64> {
    ensure(db)?;
    let sql = format!("INSERT INTO {TABLE} (job_id, rank, message, status, created_at) VALUES (?, ?, ?, 'pending', ?)");
    db.execute(&sql, &[Cell::Int(job), Cell::Int(rank), Cell::Text(message.into()), Cell::Float(now)])?;
    let rows = db.query(
        &format!("SELECT id FROM {TABLE} WHERE job_id = ? AND rank = ? ORDER BY id DESC"),
        &[Cell::Int(job), Cell::Int(rank)],
    )?;
    rows.first().map(|r| int(r, 0)).ok_or_else(|| format!("approval of job {job} not stored"))
}

/// Approval `id`, if there is one.
pub fn find(db: &mut dyn Connection, id: i64) -> Result<Option<Approval>> {
    ensure(db)?;
    let rows = db.query(&format!("SELECT {COLUMNS} FROM {TABLE} WHERE id = ?"), &[Cell::Int(id)])?;
    Ok(rows.first().map(approval))
}

/// The questions waiting, oldest first.
pub fn pending(db: &mut dyn Connection) -> Result<Vec<Approval>> {
    ensure(db)?;
    let rows = db.query(&format!("SELECT {COLUMNS} FROM {TABLE} WHERE status = 'pending' ORDER BY id"), &[])?;
    Ok(rows.iter().map(approval).collect())
}

/// The latest decisions, newest first.
pub fn decided(db: &mut dyn Connection, limit: usize) -> Result<Vec<Approval>> {
    ensure(db)?;
    let sql =
        format!("SELECT {COLUMNS} FROM {TABLE} WHERE status <> 'pending' ORDER BY decided_at DESC, id DESC LIMIT ?");
    Ok(db.query(&sql, &[Cell::Int(limit.min(10_000) as i64)])?.iter().map(approval).collect())
}

/// The questions job `job` asked, in order.
pub fn of_job(db: &mut dyn Connection, job: i64) -> Result<Vec<Approval>> {
    ensure(db)?;
    let rows = db.query(&format!("SELECT {COLUMNS} FROM {TABLE} WHERE job_id = ? ORDER BY rank"), &[Cell::Int(job)])?;
    Ok(rows.iter().map(approval).collect())
}

/// Decides pending approval `id`, and queues again the job that waited for
/// it: whether it was pending.
pub fn decide(db: &mut dyn Connection, id: i64, approved: bool, now: f64) -> Result<bool> {
    decide_by(db, id, approved, now, None)
}

/// [`decide`], recording who decided (`console`, `slack:U123`).
pub fn decide_by(db: &mut dyn Connection, id: i64, approved: bool, now: f64, by: Option<&str>) -> Result<bool> {
    ensure(db)?;
    let status = if approved { Decision::Approved } else { Decision::Denied };
    let sql =
        format!("UPDATE {TABLE} SET status = ?, decided_at = ?, decided_by = ? WHERE id = ? AND status = 'pending'");
    let by = by.map_or(Cell::Null, |by| Cell::Text(by.into()));
    if db.execute(&sql, &[Cell::Text(status.as_str().into()), Cell::Float(now), by, Cell::Int(id)])? == 0 {
        return Ok(false);
    }
    let rows = db.query(&format!("SELECT job_id FROM {TABLE} WHERE id = ?"), &[Cell::Int(id)])?;
    if let Some(row) = rows.first() {
        jobs::resume_waiting(db, int(row, 0), now)?;
    }
    Ok(true)
}
