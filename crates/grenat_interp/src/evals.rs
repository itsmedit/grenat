//! Evals: the quality of model-driven code, measured on a dataset.
//!
//! `eval "name", dataset: "rows.jsonl", threshold: 0.8 do |row| … end`
//! scores every row (a `Bool`, or a `Float` from 0 to 1, often from `judge`);
//! the eval passes when the mean score reaches the threshold. Rows run
//! concurrently; a row that fails scores 0 and its error is reported.

use std::path::PathBuf;
use std::time::Instant;

use crate::builtins::json_to_untyped;
use crate::prelude::*;
use crate::{RuntimeError, spawner};

/// Rows scored at the same time, by default.
pub(crate) const DEFAULT_CONCURRENCY: usize = 8;

/// An `eval` declared by the program.
pub(crate) struct EvalDef<'p> {
    pub name: String,
    pub dataset: String,
    pub threshold: f64,
    pub concurrency: usize,
    pub block: Value<'p>,
}

/// The score of one row.
#[derive(Debug, Clone, PartialEq)]
pub struct RowOutcome {
    pub score: f64,
    pub error: Option<RuntimeError>,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalReport {
    pub name: String,
    pub threshold: f64,
    /// In dataset order.
    pub rows: Vec<RowOutcome>,
    /// The dataset could not be read: no row was scored.
    pub error: Option<RuntimeError>,
    pub seconds: f64,
}

impl EvalReport {
    pub fn mean(&self) -> f64 {
        if self.rows.is_empty() { 0.0 } else { self.rows.iter().map(|r| r.score).sum::<f64>() / self.rows.len() as f64 }
    }

    pub fn passed(&self) -> bool {
        self.error.is_none() && !self.rows.is_empty() && self.mean() >= self.threshold
    }

    pub fn cost_usd(&self) -> f64 {
        self.rows.iter().map(|r| r.cost_usd).sum()
    }
}

/// Runs the script (which declares the evals), then each eval whose name
/// contains `filter`.
pub fn run_evals(
    program: &grenat_ast::Program,
    options: crate::Options,
    filter: Option<&str>,
) -> Result<Vec<EvalReport>, RuntimeError> {
    crate::on_interpreter_thread(|green| {
        let mut interp = Interp::new(program, options, spawner(green))?;
        if let Err(ctrl) = interp.run_script() {
            return Err(interp.runtime_error(ctrl));
        }
        let evals = std::mem::take(&mut *interp.evals.borrow_mut());
        let selected = evals.into_iter().filter(|e| filter.is_none_or(|f| e.name.contains(f)));
        let mut reports = Vec::new();
        for eval in selected {
            let report = interp.run_eval(eval);
            interp.record_eval(&report);
            reports.push(report);
        }
        Ok(reports)
    })
}

impl<'p> Interp<'p> {
    fn run_eval(&mut self, eval: EvalDef<'p>) -> EvalReport {
        let started = Instant::now();
        let mut report = EvalReport {
            name: eval.name.clone(),
            threshold: eval.threshold,
            rows: Vec::new(),
            error: None,
            seconds: 0.0,
        };
        match self.dataset(&eval.dataset) {
            Ok(rows) => report.rows = self.score_rows(rows, &eval),
            Err(ctrl) => report.error = Some(self.runtime_error(ctrl)),
        }
        self.wait_for_tasks();
        report.seconds = started.elapsed().as_secs_f64();
        report
    }

    /// The rows of a JSON Lines file (next to the program), as `Row` records.
    fn dataset(&self, name: &str) -> Result<Vec<Value<'p>>, Ctrl<'p>> {
        let path: PathBuf = self.dir.join(name);
        let text = std::fs::read_to_string(&path)
            .or_else(|e| raise("IOError", format!("cannot read the dataset {}: {e}", path.display())))?;
        let mut rows = Vec::new();
        for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let at = || format!("{}:{}", path.display(), i + 1);
            let json: serde_json::Value = serde_json::from_str(line)
                .or_else(|e| raise("ParseError", format!("invalid JSON at {}: {e}", at())))?;
            let Some(object) = json.as_object() else {
                return raise("ParseError", format!("a row must be a JSON object, at {}", at()));
            };
            let fields = object.iter().map(|(k, v)| (k.as_str().into(), json_to_untyped(v))).collect();
            rows.push(Value::record("Row", fields));
        }
        Ok(rows)
    }

    /// Scores the rows, `concurrency` at a time; results in dataset order.
    fn score_rows(&mut self, rows: Vec<Value<'p>>, eval: &EvalDef<'p>) -> Vec<RowOutcome> {
        let count = rows.len();
        let rows = Arc::new(rows);
        let next = Arc::new(AtomicUsize::new(0));
        let (sender, receiver) = grenat_green::channel();
        for _ in 0..eval.concurrency.clamp(1, count.max(1)) {
            let (rows, next, sender, block) = (rows.clone(), next.clone(), sender.clone(), eval.block.clone());
            self.spawn_task(self.fork(), move |task| {
                loop {
                    let i = next.fetch_add(1, AtomicOrdering::Relaxed);
                    if i >= count {
                        break;
                    }
                    let outcome = task.score_row(&block, rows[i].clone());
                    if sender.send((i, outcome)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut outcomes: Vec<Option<RowOutcome>> = vec![None; count];
        for (i, outcome) in receiver {
            outcomes[i] = Some(outcome);
        }
        outcomes.into_iter().map(|o| o.expect("every row is scored")).collect()
    }

    /// One row, with its own spending counter.
    fn score_row(&mut self, block: &Value<'p>, row: Value<'p>) -> RowOutcome {
        let spent = Arc::new(Budget::unlimited());
        self.budgets.push(spent.clone());
        let result = self.call_block(block, vec![row]).and_then(|v| score(&v));
        self.budgets.pop();
        let (score, error) = match result {
            Ok(score) => (score, None),
            Err(ctrl) => (0.0, Some(self.runtime_error(ctrl))),
        };
        RowOutcome { score, error, cost_usd: spent.spent() }
    }
}

/// A row's score: `true`/`false`, or a number from 0 to 1.
fn score<'p>(value: &Value<'p>) -> Result<f64, Ctrl<'p>> {
    match value.untainted() {
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Float(f) if (0.0..=1.0).contains(f) => Ok(*f),
        Value::Int(n @ (0 | 1)) => Ok(*n as f64),
        other => raise("TypeError", format!("a score is a Bool or a Float from 0 to 1, got {}", other.inspect())),
    }
}
