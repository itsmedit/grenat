//! Approvals that wait: in a job, a question to a human is stored, and the
//! job waits — for days if need be — until someone decides.
//!
//! ```ruby
//! workflow publish(id: Int) uses llm, db, human
//!   draft = step(:draft) { write_post(id) }
//!   step(:review) { approve! "Publish “#{draft.title}”?" }   # stored; the job waits
//!   step(:publish) { Blog.publish(draft) }
//! end
//!
//! Approvals.pending                           # [{"id" => 1, "message" => …}]
//! Approvals.approve(1)                        # or deny: the job runs again
//! ```
//!
//! The job runs again from the start, and a workflow replays its journaled
//! steps without running them: it finds the decision where it stopped. A
//! question is known by its job and its rank among the questions of a run,
//! so the same question gets the same answer on every run.

use grenat_db::Cell;

use crate::builtins::{cell_value, db_error};
use crate::prelude::*;

const TABLE: &str = "grenat_approvals";
/// What a job that waits for a human raises (and the worker understands).
pub(crate) const SUSPENDED: &str = "Suspended";

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

impl<'p> Interp<'p> {
    fn approvals_connection(&mut self) -> Result<crate::SharedConnection, Ctrl<'p>> {
        let Some(database) = self.app_db.borrow().clone() else {
            return raise("DbError", "approvals need a database: declare one (`database Env.fetch(\"DATABASE_URL\")`)");
        };
        let connection = self.connection_of(&database).expect("a database");
        let key = connection.lock().dialect().primary_key();
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (id {key}, job_id INTEGER NOT NULL, rank INTEGER NOT NULL, \
             message TEXT NOT NULL, status TEXT NOT NULL, created_at FLOAT NOT NULL, decided_at FLOAT)"
        );
        grenat_green::blocking(|| connection.lock().batch(&sql)).or_else(db_error)?;
        Ok(connection)
    }

    /// In a job: the stored decision on this question, or else the question
    /// stored and the job suspended.
    pub(crate) fn stored_approval(&mut self, job: i64, message: &str) -> Result<bool, Ctrl<'p>> {
        let rank = self.approval_rank.get();
        self.approval_rank.set(rank + 1);
        let connection = self.approvals_connection()?;
        let sql = format!("SELECT id, status FROM {TABLE} WHERE job_id = ? AND rank = ?");
        let params = [Cell::Int(job), Cell::Int(rank as i64)];
        let rows = grenat_green::blocking(|| connection.lock().query(&sql, &params)).or_else(db_error)?;
        let status = rows.first().and_then(|r| r.get(1)).map(|(_, c)| cell_value(c.clone()).to_display());
        match status.as_deref() {
            Some("approved") => return Ok(true),
            Some("denied") => return Ok(false),
            Some(_) => {}
            None => {
                let insert = format!("INSERT INTO {TABLE} (job_id, rank, message, status, created_at) VALUES (?, ?, ?, 'pending', ?)");
                let params = [Cell::Int(job), Cell::Int(rank as i64), Cell::Text(message.to_string()), Cell::Float(now())];
                grenat_green::blocking(|| connection.lock().execute(&insert, &params)).or_else(db_error)?;
                if self.log {
                    self.write_err(&format!("[approval] job {job} waits: {message}\n"));
                }
            }
        }
        raise(SUSPENDED, format!("waiting for a human: {message}"))
    }

    /// `Approvals.pending`: the questions waiting, oldest first.
    pub(crate) fn pending_approvals(&mut self) -> R<'p> {
        let connection = self.approvals_connection()?;
        let sql = format!("SELECT id, job_id, message, created_at FROM {TABLE} WHERE status = 'pending' ORDER BY id");
        let rows = grenat_green::blocking(|| connection.lock().query(&sql, &[])).or_else(db_error)?;
        let items = rows
            .into_iter()
            .map(|row| {
                let pairs = row.into_iter().map(|(k, c)| (Value::str(k), cell_value(c))).collect();
                Value::Hash(Arc::new(Mutex::new(pairs)))
            })
            .collect();
        Ok(Value::array(items))
    }

    /// `Approvals.approve(id)` / `Approvals.deny(id)`: the decision, and the
    /// job that waited for it queued again.
    pub(crate) fn decide(&mut self, args: &Args<'p>, approved: bool) -> R<'p> {
        let id = match args.pos.first().map(Value::untainted) {
            Some(Value::Int(n)) => *n,
            _ => return raise("ArgumentError", "`Approvals.approve` and `deny` expect the id of an approval"),
        };
        if args.pos[0].contains_taint() {
            return raise("TaintError", "an untrusted value decides an approval: check it first");
        }
        self.check_effect("human")?;
        let connection = self.approvals_connection()?;
        let status = if approved { "approved" } else { "denied" };
        let update = format!("UPDATE {TABLE} SET status = ?, decided_at = ? WHERE id = ? AND status = 'pending'");
        let params = [Cell::Text(status.into()), Cell::Float(now()), Cell::Int(id)];
        let changed = grenat_green::blocking(|| connection.lock().execute(&update, &params)).or_else(db_error)?;
        if changed == 0 {
            return raise("ArgumentError", format!("no pending approval {id}"));
        }
        let requeue = "UPDATE grenat_jobs SET status = 'queued' WHERE status = 'waiting' AND id = \
                       (SELECT job_id FROM grenat_approvals WHERE id = ?)";
        let params = [Cell::Int(id)];
        grenat_green::blocking(|| connection.lock().execute(requeue, &params)).or_else(db_error)?;
        Ok(Value::Nil)
    }
}
