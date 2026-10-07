//! A file dropped, in tests: `deliver_file("inbox/a.pdf")` hands the path —
//! untrusted, as served — to every `on_file` handler whose pattern matches
//! it, without watching anything, and says what became of it:
//! `{"status" => "handled"}`, or `{"status" => "failed", "error" =>
//! "ArgumentError: …"}` when a handler raised (served, the file would be
//! tried again). The file need not exist: a handler that reads it reads
//! the test's own fixture.

use crate::builtins::dropped_path;
use crate::prelude::*;

const USAGE: &str = "deliver_file(\"inbox/a.pdf\")";

impl<'p> Interp<'p> {
    /// `deliver_file(path)`.
    pub(crate) fn deliver_file(&mut self, args: &Args<'p>) -> R<'p> {
        self.only_in_tests("deliver_file", USAGE)?;
        let path = match (args.pos.as_slice(), args.named.is_empty()) {
            ([path], true) => match path.untainted() {
                Value::Str(path) => path.to_string(),
                Value::Secret(_) => return raise("SecretError", "a secret is no path: `deliver_file` takes a string"),
                other => {
                    let message = format!("`deliver_file` expects a path (a string), got {}", other.type_name());
                    return raise("TypeError", message);
                }
            },
            _ => return raise("ArgumentError", format!("`deliver_file` expects a path: `{USAGE}`")),
        };
        let blocks: Vec<Value<'p>> =
            self.folders.borrow().iter().filter(|f| f.glob.matches(&path)).map(|f| f.block.clone()).collect();
        if blocks.is_empty() {
            let message = if self.folders.borrow().is_empty() {
                "no folder is watched: declare `on_file \"inbox/*.pdf\" do |path| … end`".to_string()
            } else {
                format!("no `on_file` pattern matches {path:?}")
            };
            return raise("ArgumentError", message);
        }
        let mut failure = None;
        for block in blocks {
            match self.call_block(&block, vec![dropped_path(&path)]) {
                Ok(_) => {}
                Err(Ctrl::Raise(error)) => {
                    failure.get_or_insert(format!("{}: {}", error.ty, error.message));
                }
                Err(other) => return Err(other),
            }
        }
        let outcome = match failure {
            None => vec![("status", Value::str("handled"))],
            Some(error) => vec![("status", Value::str("failed")), ("error", Value::str(error))],
        };
        let pairs = outcome.into_iter().map(|(k, v)| (Value::str(k), v)).collect();
        Ok(Value::Hash(Arc::new(Mutex::new(pairs))))
    }
}
