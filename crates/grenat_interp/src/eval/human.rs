//! Human in the loop: approvals.

use crate::prelude::*;

impl<'p> Interp<'p> {
    // ── Human in the loop ────────────────────────────────

    pub(crate) fn ask_human(&mut self, message: &str) -> Result<bool, Ctrl<'p>> {
        let approver = self.approver.borrow().clone();
        match approver {
            Some(Value::Symbol(policy)) => Ok(&*policy == "approve_all"),
            Some(handler) => {
                let request = Value::record("ApprovalRequest", vec![("message".into(), Value::str(message))]);
                Ok(self.call_block(&handler, vec![request])?.truthy())
            }
            None => {
                // one question at a time, even from concurrent tasks
                let _human = self.human.borrow();
                self.write_err(&format!("\n[approval] {message}\nApprove? (y/N) "));
                let answer = self.read_line().unwrap_or_default();
                Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
            }
        }
    }
}
