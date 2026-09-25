//! Jobs: work queued in the application's database, run by the workers of
//! `grenat serve`.
//!
//! ```ruby
//! enqueue(:process_ticket, ticket.id)        # or: enqueue(:digest, in: 1.hour)
//!
//! def process_ticket(id: Int) uses llm, db   # any function; a workflow is durable
//!   …
//! end
//! ```
//!
//! A job is a function and its arguments (data, in the exact encoding of
//! workflow journals), in `grenat_jobs`. A worker claims a job with a
//! conditional update — two workers never run the same one — runs it, and
//! marks it done, or retries it later (1 min, then 2, 4…) up to `ATTEMPTS`
//! times, then marks it failed with its error. In `grenat test`, jobs wait
//! in the test's database: `Jobs.enqueued` lists them, `Jobs.perform` runs them.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use grenat_db::Cell;

use crate::builtins::{cell_value, db_error};
use crate::prelude::*;
use crate::value::codec;

const TABLE: &str = "grenat_jobs";
/// Runs of a job before it is marked failed.
const ATTEMPTS: i64 = 3;
/// How often an idle worker looks for work.
const POLL: Duration = Duration::from_secs(1);

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

impl<'p> Interp<'p> {
    fn jobs_connection(&mut self) -> Result<crate::SharedConnection, Ctrl<'p>> {
        if self.app_db.borrow().is_none() && self.offline {
            // a test that uses jobs without migrations still needs a database
            self.fresh_test_database()?;
        }
        let database = match self.app_db.borrow().clone() {
            Some(database) => database,
            None => return raise("DbError", "jobs need a database: declare one (`database Env.fetch(\"DATABASE_URL\")`)"),
        };
        let connection = self.connection_of(&database).expect("a database");
        let key = connection.lock().dialect().primary_key();
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (id {key}, name TEXT NOT NULL, args TEXT NOT NULL, \
             status TEXT NOT NULL, attempts INTEGER NOT NULL, run_at FLOAT NOT NULL, error TEXT)"
        );
        grenat_green::blocking(|| connection.lock().batch(&sql)).or_else(db_error)?;
        Ok(connection)
    }

    /// `enqueue(:name, args…, in: 1.hour)`.
    pub(crate) fn enqueue(&mut self, args: &Args<'p>) -> R<'p> {
        let Some(Value::Symbol(name)) = args.pos.first().map(Value::untainted) else {
            return raise("ArgumentError", "`enqueue` expects a function: `enqueue(:process, 42)`");
        };
        let name = name.to_string();
        if !self.fns.contains_key(name.as_str()) {
            return raise("NameError", format!("`enqueue`: unknown function `{name}`"));
        }
        if args.pos.iter().any(Value::contains_taint) {
            return raise("TaintError", format!("an untrusted value reaches the job `{name}` (effect `db.write`) without validation"));
        }
        self.check_effect("db.write")?;
        let delay = match args.named.iter().find(|(n, _)| n == "in").map(|(_, v)| v.untainted().clone()) {
            None => 0.0,
            Some(Value::Duration(s) | Value::Float(s)) => s,
            Some(Value::Int(n)) => n as f64,
            Some(other) => return raise("TypeError", format!("`in:` expects a duration, got {}", other.inspect())),
        };
        let values: Vec<serde_json::Value> =
            args.pos[1..].iter().map(codec::encode).collect::<Result<_, _>>().or_else(|e| raise("TypeError", format!("a job's arguments are data: {e}")))?;
        let connection = self.jobs_connection()?;
        let sql = format!("INSERT INTO {TABLE} (name, args, status, attempts, run_at) VALUES (?, ?, 'queued', 0, ?)");
        let params = [Cell::Text(name), Cell::Text(serde_json::Value::Array(values).to_string()), Cell::Float(now() + delay)];
        grenat_green::blocking(|| connection.lock().execute(&sql, &params)).or_else(db_error)?;
        Ok(Value::Nil)
    }

    /// `Jobs.enqueued` (tests): `[name, args]` of every queued job.
    pub(crate) fn enqueued(&mut self) -> R<'p> {
        let connection = self.jobs_connection()?;
        let sql = format!("SELECT name, args FROM {TABLE} WHERE status = 'queued' ORDER BY id");
        let rows = grenat_green::blocking(|| connection.lock().query(&sql, &[])).or_else(db_error)?;
        let mut jobs = Vec::new();
        for row in rows {
            let (name, args) = (text(&row, 0), text(&row, 1));
            let args = decode_args(&args)?;
            jobs.push(Value::array(vec![Value::Symbol(name.into()), Value::array(args)]));
        }
        Ok(Value::array(jobs))
    }

    /// Runs one job ready to run, if any; whether it found one. `all`: ready
    /// or not (tests).
    pub(crate) fn perform_one(&mut self, all: bool) -> Result<bool, Ctrl<'p>> {
        let connection = self.jobs_connection()?;
        let due = if all { f64::MAX } else { now() };
        let sql = format!("SELECT id, name, args, attempts FROM {TABLE} WHERE status = 'queued' AND run_at <= ? ORDER BY run_at, id LIMIT 1");
        let rows = grenat_green::blocking(|| connection.lock().query(&sql, &[Cell::Float(due)])).or_else(db_error)?;
        let Some(row) = rows.into_iter().next() else { return Ok(false) };
        let id = row[0].1.clone();
        // claimed only if still queued: another worker may have taken it
        let claim = format!("UPDATE {TABLE} SET status = 'running' WHERE id = ? AND status = 'queued'");
        let params = [id.clone()];
        let claimed = grenat_green::blocking(|| connection.lock().execute(&claim, &params)).or_else(db_error)?;
        if claimed == 0 {
            return Ok(true);
        }
        let (name, args, attempts) = (text(&row, 1), text(&row, 2), match cell_value(row[3].1.clone()) {
            Value::Int(n) => n,
            _ => 0,
        });
        let outcome = match (self.fns.get(name.as_str()).copied(), decode_args(&args)) {
            (Some(def), Ok(values)) => self.call_fn(def, Args { pos: values, ..Args::default() }, None).map(drop),
            (None, _) => raise("NameError", format!("unknown function `{name}`")),
            (_, Err(ctrl)) => Err(ctrl),
        };
        let (sql, params) = match outcome {
            Ok(()) => (format!("UPDATE {TABLE} SET status = 'done', attempts = ? WHERE id = ?"), vec![Cell::Int(attempts + 1), id]),
            Err(ctrl) => {
                let error = self.runtime_error(ctrl);
                let message = format!("{}: {}", error.ty, error.message);
                if self.log {
                    self.write_err(&format!("[job] {name} failed: {message}\n"));
                }
                let attempts = attempts + 1;
                if attempts >= ATTEMPTS {
                    (format!("UPDATE {TABLE} SET status = 'failed', attempts = ?, error = ? WHERE id = ?"), vec![Cell::Int(attempts), Cell::Text(message), id])
                } else {
                    let retry = now() + 60.0 * f64::from(1 << (attempts - 1).min(10) as u32);
                    (
                        format!("UPDATE {TABLE} SET status = 'queued', attempts = ?, run_at = ?, error = ? WHERE id = ?"),
                        vec![Cell::Int(attempts), Cell::Float(retry), Cell::Text(message), id],
                    )
                }
            }
        };
        grenat_green::blocking(|| connection.lock().execute(&sql, &params)).or_else(db_error)?;
        Ok(true)
    }

    /// `Jobs.perform` (tests): runs every queued job, and the jobs they queue;
    /// how many ran.
    pub(crate) fn perform_all(&mut self) -> R<'p> {
        let mut ran = 0;
        while self.perform_one(true)? {
            ran += 1;
            if ran > 10_000 {
                return raise("RuntimeError", "`Jobs.perform`: jobs keep queueing jobs");
            }
        }
        Ok(Value::Int(ran))
    }

    /// `Jobs.failed`: `[name, error]` of the jobs given up.
    pub(crate) fn failed_jobs(&mut self) -> R<'p> {
        let connection = self.jobs_connection()?;
        let sql = format!("SELECT name, error FROM {TABLE} WHERE status = 'failed' ORDER BY id");
        let rows = grenat_green::blocking(|| connection.lock().query(&sql, &[])).or_else(db_error)?;
        Ok(Value::array(rows.iter().map(|r| Value::array(vec![Value::str(text(r, 0)), Value::str(text(r, 1))])).collect()))
    }

    /// A worker of `grenat serve`: runs ready jobs, forever.
    pub(crate) fn work(&mut self) {
        loop {
            match self.perform_one(false) {
                Ok(true) => continue,
                Ok(false) => grenat_green::sleep(POLL),
                Err(ctrl) => {
                    let error = self.runtime_error(ctrl);
                    self.write_err(&format!("[jobs] {}: {}\n", error.ty, error.message));
                    grenat_green::sleep(POLL * 5);
                }
            }
        }
    }
}

fn text(row: &grenat_db::Row, i: usize) -> String {
    match row.get(i).map(|(_, c)| c) {
        Some(Cell::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

fn decode_args<'p>(text: &str) -> Result<Vec<Value<'p>>, Ctrl<'p>> {
    let json: serde_json::Value = serde_json::from_str(text).or_else(|e| raise("ParseError", format!("a job's arguments: {e}")))?;
    json.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|a| codec::decode(a).or_else(|e| raise("ParseError", e)))
        .collect()
}
