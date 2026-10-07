//! `on_file`: files dropped in a folder wake a served program.
//!
//! ```ruby
//! on_file "inbox/*.pdf", settle: 2.s do |path|
//!   invoice = extract(Pdf.read(path))     # `path` is untrusted: its name is the dropper's
//! end
//! ```
//!
//! The pattern (see `grenat_watch::Glob`: `*`, `?`, `[a-z]`, `{pdf,PDF}`,
//! `**`) is parsed when the declaration runs, and its directory held to the
//! `fs.read` capabilities. `grenat serve` watches it (see `crate::folders`);
//! in tests, `deliver_file` hands the handler a path (see
//! `eval::file_double`). The path is untrusted text: whoever drops a file
//! chooses its name (`--help.pdf`, `; rm -rf ~.pdf`), so it reaches a
//! command, a request or a written file only once checked; reading it is
//! what it is for.

use std::time::Duration;

use grenat_watch::Glob;

use crate::prelude::*;

use super::*;

/// Between two scans, when the system tells of no change.
const DEFAULT_EVERY: f64 = 10.0;

/// How long a file stays unchanged before it is handed over.
const DEFAULT_SETTLE: f64 = 2.0;

/// A folder watched by `on_file`.
pub(crate) struct FileTrigger<'p> {
    pub glob: Glob,
    /// Seconds between two scans when nothing wakes the watch; the delay
    /// before a failed run is tried again too.
    pub every: f64,
    /// Seconds a file stays unchanged before it is handed over.
    pub settle: f64,
    /// Whether the files there when `grenat serve` starts are handed over.
    pub existing: bool,
    pub block: Value<'p>,
}

impl FileTrigger<'_> {
    pub(crate) fn every(&self) -> Duration {
        Duration::from_secs_f64(self.every)
    }

    pub(crate) fn settle(&self) -> Duration {
        Duration::from_secs_f64(self.settle)
    }
}

pub(crate) fn on_file<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    const USAGE: &str = "`on_file \"inbox/*.pdf\" do |path| … end`";
    if args.pos.first().is_some_and(Value::contains_taint) {
        return raise("TaintError", "an untrusted value reaches `on_file` (effect `fs.read`) without validation");
    }
    let pattern = match args.pos.as_slice() {
        [pattern] => match pattern.untainted() {
            Value::Str(text) => text.to_string(),
            other => {
                return raise(
                    "TypeError",
                    format!("`on_file` expects a pattern (a string), got {}", other.type_name()),
                );
            }
        },
        _ => return raise("ArgumentError", format!("`on_file` expects a pattern: {USAGE}")),
    };
    let glob = Glob::parse(&pattern).or_else(|e| raise("ArgumentError", e))?;
    interp.check_fs("fs.read", &glob.base().to_string_lossy())?;
    let mut trigger = FileTrigger {
        glob,
        every: DEFAULT_EVERY,
        settle: DEFAULT_SETTLE,
        existing: true,
        block: block(args, "on_file")?,
    };
    for (option, value) in &args.named {
        match (option.as_str(), value.untainted()) {
            ("every", v) if number(v).is_some_and(|s| s > 0.0 && s.is_finite()) => {
                trigger.every = number(v).expect("checked");
            }
            ("settle", v) if number(v).is_some_and(|s| s >= 0.0 && s.is_finite()) => {
                trigger.settle = number(v).expect("checked");
            }
            ("existing", Value::Bool(existing)) => trigger.existing = *existing,
            (option, v) => {
                return raise(
                    "ArgumentError",
                    format!(
                        "invalid `on_file` option `{option}: {}` (it takes `every:` a positive duration, `settle:` a duration, `existing:` true or false)",
                        v.inspect()
                    ),
                );
            }
        }
    }
    interp.folders.borrow_mut().push(trigger);
    Ok(Value::Nil)
}

/// What a handler receives: the file's path as the pattern writes it
/// (`inbox/a.pdf`), untrusted.
pub(crate) fn dropped_path<'p>(path: &str) -> Value<'p> {
    Value::str(path).taint()
}
