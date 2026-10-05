//! The clock `Time.now` and `Time.today` read, and `freeze_time`: in a
//! test, `freeze_time("2026-09-28T08:00:00Z") do … end` stops it at that
//! instant for the block (nested blocks too; the clock that was is back
//! after, also on a raise). Outside a test the clock is never stopped.

use crate::builtins::iso8601;
use crate::prelude::*;

impl<'p> Interp<'p> {
    /// Seconds since the epoch: those `freeze_time` gives, or the system's.
    pub(crate) fn now(&self) -> f64 {
        match *self.frozen_clock.borrow() {
            Some(t) => t,
            None => {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()
            }
        }
    }

    /// `freeze_time("2026-09-28T08:00:00Z") do … end` (or epoch seconds): the block's value.
    pub(crate) fn freeze_time(&mut self, args: &Args<'p>) -> R<'p> {
        if !self.testing.load(AtomicOrdering::SeqCst) {
            return raise(
                "RuntimeError",
                "`freeze_time` only works in a test: `test \"…\" do freeze_time(…) do … end end`",
            );
        }
        let instant = match args.pos.first().map(Value::untainted) {
            Some(Value::Str(text)) => iso8601::parse(text).or_else(|e| raise("ArgumentError", e))?,
            Some(Value::Int(n)) => *n as f64,
            Some(Value::Float(t)) if t.is_finite() => *t,
            _ => {
                return raise(
                    "ArgumentError",
                    "`freeze_time` expects an instant: `freeze_time(\"2026-09-28T08:00:00Z\") do … end` (or epoch seconds)",
                );
            }
        };
        let body = builtins::block(args, "freeze_time")?;
        let saved = self.frozen_clock.borrow_mut().replace(instant);
        let result = self.call_block(&body, Vec::new());
        *self.frozen_clock.borrow_mut() = saved;
        result
    }
}
