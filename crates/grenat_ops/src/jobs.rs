//! Jobs (`grenat_jobs`): a function and its arguments, queued. A worker
//! claims one with a conditional update — two workers never run the same —
//! then marks it done, waiting (for a human), queued again later, or failed.

use grenat_db::{Cell, Connection};

use crate::Result;
use crate::row::{float, int, opt_float, opt_text, text};

pub const TABLE: &str = "grenat_jobs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    /// Waiting for a human's decision (see [`crate::approvals`]).
    Waiting,
    Done,
    Failed,
}

impl Status {
    pub const ALL: [Status; 5] = [Status::Queued, Status::Running, Status::Waiting, Status::Done, Status::Failed];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Waiting => "waiting",
            Status::Done => "done",
            Status::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|s| s.as_str() == text)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: i64,
    /// The function run.
    pub name: String,
    /// Its arguments: a JSON array, in the runtime's encoding.
    pub args: String,
    pub status: Status,
    /// Runs so far.
    pub attempts: i64,
    /// When it may run (next).
    pub run_at: f64,
    /// The error of its last failed run.
    pub error: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
}

const COLUMNS: &str = "id, name, args, status, attempts, run_at, error, created_at, updated_at";

fn job(row: &grenat_db::Row) -> Job {
    Job {
        id: int(row, 0),
        name: text(row, 1),
        args: text(row, 2),
        status: Status::parse(&text(row, 3)).unwrap_or(Status::Failed),
        attempts: int(row, 4),
        run_at: float(row, 5),
        error: opt_text(row, 6),
        created_at: opt_float(row, 7).unwrap_or(0.0),
        updated_at: opt_float(row, 8).unwrap_or(0.0),
    }
}

pub fn ensure(db: &mut dyn Connection) -> Result<()> {
    let key = db.dialect().primary_key();
    db.batch(&format!(
        "CREATE TABLE IF NOT EXISTS {TABLE} (id {key}, name TEXT NOT NULL, args TEXT NOT NULL, \
         status TEXT NOT NULL, attempts INTEGER NOT NULL, run_at FLOAT NOT NULL, error TEXT, \
         created_at FLOAT, updated_at FLOAT)"
    ))
}

/// Queues `name(args…)` to run at `run_at`; its id.
pub fn enqueue(db: &mut dyn Connection, name: &str, args: &str, run_at: f64, now: f64) -> Result<i64> {
    ensure(db)?;
    let sql = format!(
        "INSERT INTO {TABLE} (name, args, status, attempts, run_at, created_at, updated_at) \
         VALUES (?, ?, 'queued', 0, ?, ?, ?) RETURNING id"
    );
    let params = [Cell::Text(name.into()), Cell::Text(args.into()), Cell::Float(run_at), Cell::Float(now), Cell::Float(now)];
    let rows = db.query(&sql, &params)?;
    Ok(rows.first().map_or(0, |r| int(r, 0)))
}

pub fn get(db: &mut dyn Connection, id: i64) -> Result<Option<Job>> {
    ensure(db)?;
    let rows = db.query(&format!("SELECT {COLUMNS} FROM {TABLE} WHERE id = ?"), &[Cell::Int(id)])?;
    Ok(rows.first().map(job))
}

/// Jobs, newest first: all, or those with `status`.
pub fn list(db: &mut dyn Connection, status: Option<Status>, limit: usize) -> Result<Vec<Job>> {
    ensure(db)?;
    let limit = limit.min(10_000) as i64;
    let rows = match status {
        Some(status) => db.query(
            &format!("SELECT {COLUMNS} FROM {TABLE} WHERE status = ? ORDER BY id DESC LIMIT ?"),
            &[Cell::Text(status.as_str().into()), Cell::Int(limit)],
        )?,
        None => db.query(&format!("SELECT {COLUMNS} FROM {TABLE} ORDER BY id DESC LIMIT ?"), &[Cell::Int(limit)])?,
    };
    Ok(rows.iter().map(job).collect())
}

/// The queued jobs, oldest first.
pub fn queued(db: &mut dyn Connection) -> Result<Vec<Job>> {
    ensure(db)?;
    let rows = db.query(&format!("SELECT {COLUMNS} FROM {TABLE} WHERE status = 'queued' ORDER BY id"), &[])?;
    Ok(rows.iter().map(job).collect())
}

/// How many jobs have each status.
pub fn counts(db: &mut dyn Connection) -> Result<Vec<(Status, i64)>> {
    ensure(db)?;
    let rows = db.query(&format!("SELECT status, count(*) FROM {TABLE} GROUP BY status"), &[])?;
    let found: Vec<(String, i64)> = rows.iter().map(|r| (text(r, 0), int(r, 1))).collect();
    Ok(Status::ALL
        .into_iter()
        .map(|s| (s, found.iter().find(|(name, _)| name == s.as_str()).map_or(0, |(_, n)| *n)))
        .collect())
}

/// The next queued job due by `due`, not claimed yet.
pub fn next_due(db: &mut dyn Connection, due: f64) -> Result<Option<Job>> {
    ensure(db)?;
    let sql = format!("SELECT {COLUMNS} FROM {TABLE} WHERE status = 'queued' AND run_at <= ? ORDER BY run_at, id LIMIT 1");
    Ok(db.query(&sql, &[Cell::Float(due)])?.first().map(job))
}

/// Takes job `id` if it is still queued: whether this worker got it.
pub fn claim(db: &mut dyn Connection, id: i64, now: f64) -> Result<bool> {
    let sql = format!("UPDATE {TABLE} SET status = 'running', updated_at = ? WHERE id = ? AND status = 'queued'");
    Ok(db.execute(&sql, &[Cell::Float(now), Cell::Int(id)])? == 1)
}

pub fn finish(db: &mut dyn Connection, id: i64, attempts: i64, now: f64) -> Result<()> {
    let sql = format!("UPDATE {TABLE} SET status = 'done', attempts = ?, updated_at = ? WHERE id = ?");
    db.execute(&sql, &[Cell::Int(attempts), Cell::Float(now), Cell::Int(id)]).map(drop)
}

/// Job `id` waits for a human.
pub fn wait(db: &mut dyn Connection, id: i64, now: f64) -> Result<()> {
    let sql = format!("UPDATE {TABLE} SET status = 'waiting', updated_at = ? WHERE id = ?");
    db.execute(&sql, &[Cell::Float(now), Cell::Int(id)]).map(drop)
}

/// Job `id` failed, and is given up.
pub fn fail(db: &mut dyn Connection, id: i64, attempts: i64, error: &str, now: f64) -> Result<()> {
    let sql = format!("UPDATE {TABLE} SET status = 'failed', attempts = ?, error = ?, updated_at = ? WHERE id = ?");
    db.execute(&sql, &[Cell::Int(attempts), Cell::Text(error.into()), Cell::Float(now), Cell::Int(id)]).map(drop)
}

/// Job `id` failed, and runs again at `run_at`.
pub fn retry_later(db: &mut dyn Connection, id: i64, attempts: i64, run_at: f64, error: &str, now: f64) -> Result<()> {
    let sql = format!("UPDATE {TABLE} SET status = 'queued', attempts = ?, run_at = ?, error = ?, updated_at = ? WHERE id = ?");
    let params = [Cell::Int(attempts), Cell::Float(run_at), Cell::Text(error.into()), Cell::Float(now), Cell::Int(id)];
    db.execute(&sql, &params).map(drop)
}

/// A failed job queued again now, its attempts counted afresh (a workflow
/// resumes from its journal): whether there was such a job.
pub fn retry(db: &mut dyn Connection, id: i64, now: f64) -> Result<bool> {
    ensure(db)?;
    let sql = format!("UPDATE {TABLE} SET status = 'queued', attempts = 0, run_at = ?, updated_at = ? WHERE id = ? AND status = 'failed'");
    Ok(db.execute(&sql, &[Cell::Float(now), Cell::Float(now), Cell::Int(id)])? == 1)
}

/// Job `job`, if it waits for a human, queued again.
pub(crate) fn resume_waiting(db: &mut dyn Connection, job: i64, now: f64) -> Result<()> {
    ensure(db)?;
    let sql = format!("UPDATE {TABLE} SET status = 'queued', updated_at = ? WHERE id = ? AND status = 'waiting'");
    db.execute(&sql, &[Cell::Float(now), Cell::Int(job)]).map(drop)
}
