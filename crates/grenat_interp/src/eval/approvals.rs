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
//!
//! With `approvals_in_slack`, a question stored is also posted to a Slack
//! channel, with buttons that decide it (see `eval::slack_approvals`).

use grenat_ops::approvals::{self, Decision};

use crate::eval::store::now;
use crate::prelude::*;

/// What a job that waits for a human raises (and the worker understands).
pub(crate) const SUSPENDED: &str = "Suspended";

impl<'p> Interp<'p> {
    /// In a job: the stored decision on this question, or else the question
    /// stored and the job suspended.
    pub(crate) fn stored_approval(&mut self, job: i64, message: &str) -> Result<bool, Ctrl<'p>> {
        let rank = self.approval_rank.get() as i64;
        self.approval_rank.set(rank as usize + 1);
        match self.store("approvals", |db| approvals::decision(db, job, rank))? {
            Some(Decision::Approved) => return Ok(true),
            Some(Decision::Denied) => return Ok(false),
            Some(Decision::Pending) => {}
            None => {
                let id = self.store("approvals", |db| approvals::ask(db, job, rank, message, now()))?;
                if self.log {
                    self.write_err(&format!("[approval] job {job} waits: {message}\n"));
                }
                self.post_approval(id, job, message);
            }
        }
        raise(SUSPENDED, format!("waiting for a human: {message}"))
    }

    /// `Approvals.pending`: the questions waiting, oldest first.
    pub(crate) fn pending_approvals(&mut self) -> R<'p> {
        let pending = self.store("approvals", approvals::pending)?;
        let items = pending
            .into_iter()
            .map(|a| {
                let pairs = vec![
                    (Value::str("id"), Value::Int(a.id)),
                    (Value::str("job_id"), Value::Int(a.job_id)),
                    (Value::str("message"), Value::str(a.message)),
                    (Value::str("created_at"), Value::Float(a.created_at)),
                ];
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
        if !self.decide_approval(id, approved, None)? {
            return raise("ArgumentError", format!("no pending approval {id}"));
        }
        Ok(Value::Nil)
    }

    /// Decides pending approval `id` — recording who decided (`slack:U123`;
    /// `None`: the program) — and queues again the job that waited for it:
    /// whether it was pending. `Approvals.approve`, `deny` and a click in
    /// Slack all decide here.
    pub(crate) fn decide_approval(&mut self, id: i64, approved: bool, by: Option<&str>) -> Result<bool, Ctrl<'p>> {
        let decided = self.store("approvals", |db| approvals::decide_by(db, id, approved, now(), by))?;
        if decided && self.log {
            let decision = if approved { "approved" } else { "denied" };
            self.write_err(&format!(
                "[approval] {id} {decision}{}\n",
                by.map(|b| format!(" by {b}")).unwrap_or_default()
            ));
        }
        Ok(decided)
    }
}
