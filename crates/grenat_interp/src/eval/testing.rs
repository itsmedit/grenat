//! What only a test may do: stop the clock (`freeze_time`), give the
//! environment (`mock_env`), fail the mail (`mock_mail`), read the requests
//! sent (`Http.requests`). Outside a `test` block, even while a test file
//! loads, they are refused: a program never runs with them.

use crate::prelude::*;

impl<'p> Interp<'p> {
    /// Whether a test's block is running.
    pub(crate) fn in_test(&self) -> bool {
        self.testing.load(AtomicOrdering::SeqCst)
    }

    /// Refuses `what` outside a test; `usage` shows it in one.
    pub(crate) fn only_in_tests(&self, what: &str, usage: &str) -> Result<(), Ctrl<'p>> {
        if self.in_test() {
            return Ok(());
        }
        raise("RuntimeError", format!("`{what}` only works in a test: `test \"…\" do {usage} end`"))
    }
}
