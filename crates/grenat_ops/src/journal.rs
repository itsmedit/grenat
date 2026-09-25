//! Workflow journals: one JSON Lines file per run of a workflow — a run
//! being a workflow and its arguments — named `<workflow>-<hash>.jsonl`.
//! Each completed step is a line `{"step": name, "n": occurrence, "value": …}`,
//! appended and synced; a completed run ends with `{"result": …}`.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use crate::Result;

/// A step journaled.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub name: String,
    /// Its occurrence among the steps of that name (0 for the first).
    pub n: usize,
    /// Its value, in the runtime's encoding.
    pub value: Json,
}

/// A journal as read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Journal {
    pub steps: Vec<Step>,
    /// The workflow's result, when the run completed.
    pub result: Option<Json>,
}

/// A journal on disk, for listing.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The workflow.
    pub workflow: String,
    /// The file's name without `.jsonl`: identifies the run.
    pub run: String,
    pub path: PathBuf,
    /// When it was last written (seconds since the epoch).
    pub modified: f64,
}

/// FNV-1a: a stable hash of the arguments, naming the run.
pub fn fingerprint(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The journal of `workflow` called with `args` (positional, encoded) and
/// named arguments `names` (their values last in `args`).
pub fn path(dir: &Path, workflow: &str, args: &[Json], names: &[&str]) -> PathBuf {
    let id = fingerprint(&json!([args, names]).to_string());
    dir.join(format!("{workflow}-{id:016x}.jsonl"))
}

/// Reads a journal; a missing file is an empty journal, a line cut by a
/// crash is ignored (its step runs again).
pub fn read(path: &Path) -> Result<Journal> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Journal::default()),
        Err(e) => return Err(format!("cannot read the journal {}: {e}", path.display())),
    };
    let mut journal = Journal::default();
    for entry in text.lines().filter_map(|l| serde_json::from_str::<Json>(l).ok()) {
        if let Some(step) = entry["step"].as_str() {
            let n = entry["n"].as_u64().unwrap_or(0) as usize;
            journal.steps.push(Step { name: step.to_string(), n, value: entry["value"].clone() });
        } else if let Some(result) = entry.get("result") {
            journal.result = Some(result.clone());
        }
    }
    Ok(journal)
}

fn append(path: &Path, entry: &Json) -> Result<()> {
    let fail = |e: std::io::Error| format!("cannot write the journal {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(fail)?;
    writeln!(file, "{entry}").map_err(fail)?;
    file.sync_data().map_err(fail)
}

pub fn append_step(path: &Path, name: &str, n: usize, value: &Json) -> Result<()> {
    append(path, &json!({"step": name, "n": n, "value": value}))
}

pub fn append_result(path: &Path, result: &Json) -> Result<()> {
    append(path, &json!({"result": result}))
}

/// The journals in `dir`, most recently written first.
pub fn list(dir: &Path) -> Vec<Entry> {
    let Ok(files) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut entries: Vec<Entry> = files
        .flatten()
        .filter_map(|file| {
            let path = file.path();
            let run = path.file_name()?.to_str()?.strip_suffix(".jsonl")?.to_string();
            // `<workflow>-<16 hex digits>`
            let (workflow, hash) = run.rsplit_once('-')?;
            if hash.len() != 16 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let modified = file.metadata().ok()?.modified().ok()?;
            let modified = modified.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            Some(Entry { workflow: workflow.to_string(), run: run.clone(), path, modified })
        })
        .collect();
    entries.sort_by(|a, b| b.modified.total_cmp(&a.modified).then_with(|| a.run.cmp(&b.run)));
    entries
}

/// The journal of `run` (as [`Entry::run`] names it) in `dir`, if it is one.
pub fn find(dir: &Path, run: &str) -> Option<Entry> {
    list(dir).into_iter().find(|e| e.run == run)
}
