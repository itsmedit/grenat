//! Durable workflows: every `step` of a `workflow` is journaled, so that a
//! run interrupted (a crash, a redeploy, an error) resumes where it stopped:
//! completed steps are replayed from the journal — no model call is billed
//! twice — and a completed workflow returns its recorded result.
//!
//! A run is identified by the workflow and its arguments. Its journal is a
//! JSON Lines file (`<journal dir>/<name>-<hash>.jsonl`), appended and
//! synced step by step. Steps are numbered by name, in the order they run.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use grenat_ops::journal;
use serde_json::Value as Json;

use crate::prelude::*;
use crate::value::codec;

/// A running workflow and its journal.
pub(crate) struct WorkflowRun {
    name: String,
    path: PathBuf,
    /// Steps journaled by earlier runs, by (name, occurrence).
    done: HashMap<(String, usize), Json>,
    /// The result, if an earlier run completed.
    result: Option<Json>,
    /// Occurrences of each step name in this run.
    counts: Mutex<HashMap<String, usize>>,
}

impl WorkflowRun {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    fn open(name: &str, path: PathBuf) -> Result<WorkflowRun, String> {
        let read = journal::read(&path)?;
        Ok(WorkflowRun {
            name: name.to_string(),
            path,
            done: read.steps.into_iter().map(|s| ((s.name, s.n), s.value)).collect(),
            result: read.result,
            counts: Mutex::new(HashMap::new()),
        })
    }
}

impl<'p> Interp<'p> {
    /// The run of `def` for these arguments (resumed if journaled).
    pub(crate) fn open_workflow(&mut self, def: &'p FnDef, args: &Args<'p>) -> Result<Arc<WorkflowRun>, Ctrl<'p>> {
        let (values, names) = run_key(def, args);
        let mut key = Vec::new();
        for value in values {
            match codec::encode(value) {
                Ok(json) => key.push(json),
                Err(why) => {
                    return raise(
                        "TypeError",
                        format!("the arguments of workflow `{}` must be data: {why}", def.name.name),
                    );
                }
            }
        }
        let path = journal::path(&self.journal_dir.borrow(), &def.name.name, &key, &names);
        match WorkflowRun::open(&def.name.name, path) {
            Ok(run) => Ok(Arc::new(run)),
            Err(e) => raise("JournalError", e),
        }
    }

    /// Runs a workflow's body, or returns the result of a completed run,
    /// in its span (`invoke_workflow <name>`).
    pub(crate) fn run_workflow(&mut self, def: &'p FnDef, run: Arc<WorkflowRun>) -> R<'p> {
        let start = |_: &Self| {
            grenat_otel::SpanStart::new(format!("invoke_workflow {}", run.name), grenat_otel::Kind::Internal)
                .attr("gen_ai.operation.name", "invoke_workflow")
                .attr("gen_ai.workflow.name", run.name.as_str())
                .attr("grenat.workflow.journaled_steps", run.done.len())
                .attr("grenat.workflow.replayed", run.result.is_some())
        };
        let traced = run.clone();
        self.traced(start, |interp| interp.run_workflow_body(def, traced))
    }

    fn run_workflow_body(&mut self, def: &'p FnDef, run: Arc<WorkflowRun>) -> R<'p> {
        if let Some(result) = &run.result {
            if self.log {
                self.write_err(&format!("[workflow] {}: completed earlier, result replayed\n", run.name));
            }
            return codec::decode(result).or_else(|e| raise("JournalError", e));
        }
        if self.log && !run.done.is_empty() {
            self.write_err(&format!("[workflow] {}: resumed, {} step(s) journaled\n", run.name, run.done.len()));
        }
        self.workflows.push(run.clone());
        let result = self.eval_body(&def.body);
        self.workflows.pop();
        let value = match result {
            Ok(v) | Err(Ctrl::Return(v)) => v,
            Err(other) => return Err(other),
        };
        let json = codec::encode(&value)
            .or_else(|why| raise("TypeError", format!("the result of workflow `{}` must be data: {why}", run.name)))?;
        journal::append_result(&run.path, &json).or_else(|e| raise("JournalError", e))?;
        Ok(value)
    }

    /// `step(:name) { … }`: in a workflow, journaled (or replayed); elsewhere, just run.
    pub(crate) fn step(&mut self, name: &str, block: &Value<'p>) -> R<'p> {
        let Some(run) = self.workflows.last().cloned() else {
            return self.call_block(block, Vec::new());
        };
        let n = {
            let mut counts = run.counts.borrow_mut();
            let count = counts.entry(name.to_string()).or_insert(0);
            *count += 1;
            *count - 1
        };
        let replayed = run.done.get(&(name.to_string(), n));
        let start = |_: &Self| {
            grenat_otel::SpanStart::new(format!("step {name}"), grenat_otel::Kind::Internal)
                .attr("grenat.workflow.name", run.name.as_str())
                .attr("grenat.step.name", name)
                .attr("grenat.step.occurrence", n)
                .attr("grenat.step.replayed", replayed.is_some())
        };
        self.traced(start, |interp| interp.run_step(&run, name, n, replayed, block))
    }

    /// A step's block run and journaled, or its journaled value replayed.
    fn run_step(
        &mut self,
        run: &WorkflowRun,
        name: &str,
        n: usize,
        replayed: Option<&Json>,
        block: &Value<'p>,
    ) -> R<'p> {
        if let Some(json) = replayed {
            if self.log {
                self.write_err(&format!("[workflow] {}: step :{name} replayed\n", run.name));
            }
            return codec::decode(json).or_else(|e| raise("JournalError", e));
        }
        let value = self.call_block(block, Vec::new())?;
        let json = codec::encode(&value)
            .or_else(|why| raise("TypeError", format!("step :{name} must return data to be journaled: {why}")))?;
        journal::append_step(&run.path, name, n, &json).or_else(|e| raise("JournalError", e))?;
        Ok(value)
    }
}

/// What identifies a run: its arguments in the order of the parameters, as
/// they bind, so that `weekly("2026-09-28")`, `weekly(monday: "2026-09-28")`
/// and a job queued either way are one run, whatever the keywords' order.
/// Values for the parameters up to the first left to its default are
/// positional, unnamed — a call written positionally keeps the key it always
/// had; those after it are named. What binds to no parameter (an error when
/// the body runs) follows, as written.
fn run_key<'a, 'p>(def: &'a FnDef, args: &'a Args<'p>) -> (Vec<&'a Value<'p>>, Vec<&'a str>) {
    let mut pos = args.pos.iter();
    let mut used = vec![false; args.named.len()];
    let (mut values, mut names) = (Vec::new(), Vec::new());
    let mut gap = false;
    for param in &def.params {
        let named = args.named.iter().position(|(n, _)| *n == param.name.name);
        let value = match named {
            Some(i) => {
                used[i] = true;
                Some(&args.named[i].1)
            }
            None => pos.next(),
        };
        match value {
            Some(v) if gap => {
                values.push(v);
                names.push(param.name.name.as_str());
            }
            Some(v) => values.push(v),
            None => gap = true,
        }
    }
    values.extend(pos);
    for (i, (n, v)) in args.named.iter().enumerate() {
        if !used[i] {
            values.push(v);
            names.push(n.as_str());
        }
    }
    (values, names)
}
