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

use std::time::Duration;

use grenat_ops::jobs;

use crate::eval::store::now;
use crate::prelude::*;
use crate::value::codec;

/// Runs of a job before it is marked failed.
const ATTEMPTS: i64 = 3;
/// How often an idle worker looks for work.
const POLL: Duration = Duration::from_secs(1);

impl<'p> Interp<'p> {
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
        let encoded = serde_json::Value::Array(values).to_string();
        let now = now();
        self.store("jobs", |db| jobs::enqueue(db, &name, &encoded, now + delay, now))?;
        Ok(Value::Nil)
    }

    /// `Jobs.enqueued` (tests): `[name, args]` of every queued job.
    pub(crate) fn enqueued(&mut self) -> R<'p> {
        let queued = self.store("jobs", jobs::queued)?;
        let mut items = Vec::new();
        for job in queued {
            let args = decode_args(&job.args)?;
            items.push(Value::array(vec![Value::Symbol(job.name.into()), Value::array(args)]));
        }
        Ok(Value::array(items))
    }

    /// Runs one job ready to run, if any; whether it found one. `all`: ready
    /// or not (tests).
    pub(crate) fn perform_one(&mut self, all: bool) -> Result<bool, Ctrl<'p>> {
        let due = if all { f64::MAX } else { now() };
        let Some(job) = self.store("jobs", |db| jobs::next_due(db, due))? else { return Ok(false) };
        // claimed only if still queued: another worker may have taken it
        if !self.store("jobs", |db| jobs::claim(db, job.id, now()))? {
            return Ok(true);
        }
        let (outer_job, outer_rank) = (self.current_job, self.approval_rank.get());
        self.current_job = Some(job.id);
        self.approval_rank.set(0);
        let outcome = match (self.fns.get(job.name.as_str()).copied(), decode_args(&job.args)) {
            (Some(def), Ok(values)) => self.call_fn(def, Args { pos: values, ..Args::default() }, None).map(drop),
            (None, _) => raise("NameError", format!("unknown function `{}`", job.name)),
            (_, Err(ctrl)) => Err(ctrl),
        };
        self.current_job = outer_job;
        self.approval_rank.set(outer_rank);
        let attempts = job.attempts + 1;
        match outcome {
            Ok(()) => self.store("jobs", |db| jobs::finish(db, job.id, attempts, now()))?,
            // waiting for a human: neither done nor failed
            Err(ctrl) if ctrl.error_type() == Some(crate::eval::approvals::SUSPENDED) => {
                self.store("jobs", |db| jobs::wait(db, job.id, now()))?;
            }
            Err(ctrl) => {
                let error = self.runtime_error(ctrl);
                let message = format!("{}: {}", error.ty, error.message);
                if self.log {
                    self.write_err(&format!("[job] {} failed: {message}\n", job.name));
                }
                self.record_event("job", &format!("job {} ({})", job.id, job.name), &error);
                // a human said no: asking again would not change it
                if attempts >= ATTEMPTS || error.ty == "ApprovalDenied" {
                    self.store("jobs", |db| jobs::fail(db, job.id, attempts, &message, now()))?;
                } else {
                    let retry = now() + 60.0 * f64::from(1 << (attempts - 1).min(10) as u32);
                    self.store("jobs", |db| jobs::retry_later(db, job.id, attempts, retry, &message, now()))?;
                }
            }
        }
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
        let mut failed = self.store("jobs", |db| jobs::list(db, Some(jobs::Status::Failed), 10_000))?;
        failed.reverse();
        let items = failed
            .into_iter()
            .map(|job| Value::array(vec![Value::str(job.name), Value::str(job.error.unwrap_or_default())]))
            .collect();
        Ok(Value::array(items))
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

fn decode_args<'p>(text: &str) -> Result<Vec<Value<'p>>, Ctrl<'p>> {
    let json: serde_json::Value = serde_json::from_str(text).or_else(|e| raise("ParseError", format!("a job's arguments: {e}")))?;
    json.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|a| codec::decode(a).or_else(|e| raise("ParseError", e)))
        .collect()
}
