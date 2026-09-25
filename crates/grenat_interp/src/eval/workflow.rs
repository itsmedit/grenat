//! Durable workflows: every `step` of a `workflow` is journaled, so that a
//! run interrupted (a crash, a redeploy, an error) resumes where it stopped:
//! completed steps are replayed from the journal — no model call is billed
//! twice — and a completed workflow returns its recorded result.
//!
//! A run is identified by the workflow and its arguments. Its journal is a
//! JSON Lines file (`<journal dir>/<name>-<hash>.jsonl`), appended and
//! synced step by step. Steps are numbered by name, in the order they run.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value as Json, json};

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
    fn open(name: &str, path: PathBuf) -> Result<WorkflowRun, String> {
        let mut run = WorkflowRun {
            name: name.to_string(),
            path,
            done: HashMap::new(),
            result: None,
            counts: Mutex::new(HashMap::new()),
        };
        let text = match std::fs::read_to_string(&run.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(run),
            Err(e) => return Err(format!("cannot read the journal {}: {e}", run.path.display())),
        };
        // a line cut by a crash is ignored: its step runs again
        for entry in text.lines().filter_map(|l| serde_json::from_str::<Json>(l).ok()) {
            if let Some(step) = entry["step"].as_str() {
                let n = entry["n"].as_u64().unwrap_or(0) as usize;
                run.done.insert((step.to_string(), n), entry["value"].clone());
            } else if entry.get("result").is_some() {
                run.result = Some(entry["result"].clone());
            }
        }
        Ok(run)
    }

    fn append(&self, entry: Json) -> Result<(), String> {
        let fail = |e: std::io::Error| format!("cannot write the journal {}: {e}", self.path.display());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path).map_err(fail)?;
        writeln!(file, "{entry}").map_err(fail)?;
        file.sync_data().map_err(fail)
    }
}

/// FNV-1a: a stable hash of the arguments, naming the run.
fn fingerprint(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

impl<'p> Interp<'p> {
    /// The run of `def` for these arguments (resumed if journaled).
    pub(crate) fn open_workflow(&mut self, def: &'p FnDef, args: &Args<'p>) -> Result<Arc<WorkflowRun>, Ctrl<'p>> {
        let mut key = Vec::new();
        for value in args.pos.iter().chain(args.named.iter().map(|(_, v)| v)) {
            match codec::encode(value) {
                Ok(json) => key.push(json),
                Err(why) => {
                    return raise("TypeError", format!("the arguments of workflow `{}` must be data: {why}", def.name.name));
                }
            }
        }
        let names: Vec<&str> = args.named.iter().map(|(n, _)| n.as_str()).collect();
        let id = fingerprint(&json!([key, names]).to_string());
        let path = self.journal_dir.join(format!("{}-{id:016x}.jsonl", def.name.name));
        match WorkflowRun::open(&def.name.name, path) {
            Ok(run) => Ok(Arc::new(run)),
            Err(e) => raise("JournalError", e),
        }
    }

    /// Runs a workflow's body, or returns the result of a completed run.
    pub(crate) fn run_workflow(&mut self, def: &'p FnDef, run: Arc<WorkflowRun>) -> R<'p> {
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
        let json = codec::encode(&value).or_else(|why| {
            raise("TypeError", format!("the result of workflow `{}` must be data: {why}", run.name))
        })?;
        run.append(json!({"result": json})).or_else(|e| raise("JournalError", e))?;
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
        if let Some(json) = run.done.get(&(name.to_string(), n)) {
            if self.log {
                self.write_err(&format!("[workflow] {}: step :{name} replayed\n", run.name));
            }
            return codec::decode(json).or_else(|e| raise("JournalError", e));
        }
        let value = self.call_block(block, Vec::new())?;
        let json = codec::encode(&value).or_else(|why| {
            raise("TypeError", format!("step :{name} must return data to be journaled: {why}"))
        })?;
        run.append(json!({"step": name, "n": n, "value": json})).or_else(|e| raise("JournalError", e))?;
        Ok(value)
    }
}
