//! What the runtime records for `grenat console`, in the application's
//! database when it has one: each model call and its cost (on behalf of
//! which agent, workflow and job), what failed while serving — refusals
//! included — and eval runs. Recording never fails a program: a write that
//! fails is reported, and the program goes on.

use grenat_ops::calls::Call;
use grenat_ops::{calls, evals, events};

use crate::eval::store::now;
use crate::evals::EvalReport;
use crate::RuntimeError;
use crate::prelude::*;

impl<'p> Interp<'p> {
    /// Runs `f` on the application's database, if it has one.
    fn ledger(&mut self, f: impl FnOnce(&mut dyn grenat_db::Connection) -> Result<(), String> + Send) {
        let Some(database) = self.app_db.borrow().clone() else { return };
        let Some(connection) = self.connection_of(&database) else { return };
        if let Err(e) = grenat_green::blocking(|| f(&mut **connection.lock())) {
            self.write_err(&format!("warning: not recorded for the console: {e}\n"));
        }
    }

    /// A model call, on behalf of the running agent, workflow and job.
    pub(crate) fn record_call(&mut self, model: &str, usage: &grenat_llm::Usage, cost_usd: f64) {
        let call = Call {
            at: now(),
            model: model.to_string(),
            agent: self.agents.last().map(|frame| frame.agent.ty.to_string()),
            workflow: self.workflows.last().map(|run| run.name().to_string()),
            job_id: self.current_job,
            input_tokens: (usage.input_tokens + usage.cache_creation_input_tokens + usage.cache_read_input_tokens) as i64,
            output_tokens: usage.output_tokens as i64,
            cost_usd,
        };
        self.ledger(|db| calls::record(db, &call));
    }

    /// Something that failed while serving: `source` is `request`,
    /// `schedule`, `job` or `mcp`, `subject` which one.
    pub(crate) fn record_event(&mut self, source: &str, subject: &str, error: &RuntimeError) {
        let (ty, message) = (error.ty.clone(), error.message.clone());
        self.ledger(|db| events::record(db, now(), source, subject, &ty, &message));
    }

    pub(crate) fn record_eval(&mut self, report: &EvalReport) {
        let run = evals::Run {
            at: now(),
            name: report.name.clone(),
            score: report.mean(),
            threshold: report.threshold,
            passed: report.passed(),
            rows: report.rows.len() as i64,
            failed_rows: report.rows.iter().filter(|r| r.error.is_some()).count() as i64,
            cost_usd: report.cost_usd(),
            seconds: report.seconds,
            ..evals::Run::default()
        };
        self.ledger(|db| evals::record(db, &run));
    }
}
