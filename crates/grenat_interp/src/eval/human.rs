//! Humain dans la boucle : approbations.

use crate::prelude::*;

impl<'p> Interp<'p> {
    // ── Humain dans la boucle ────────────────────────────────

    pub(crate) fn ask_human(&mut self, message: &str) -> Result<bool, Ctrl<'p>> {
        let approver = self.approver.borrow().clone();
        match approver {
            Some(Value::Symbol(policy)) => Ok(&*policy == "approve_all"),
            Some(handler) => {
                let request = Value::record("ApprovalRequest", vec![("message".into(), Value::str(message))]);
                Ok(self.call_block(&handler, vec![request])?.truthy())
            }
            None => {
                // une seule question à la fois, même depuis des tâches concurrentes
                let _human = self.human.borrow();
                self.write_err(&format!("\n[approbation] {message}\nApprouver ? (o/N) "));
                let answer = self.read_line().unwrap_or_default();
                Ok(matches!(answer.trim().to_lowercase().as_str(), "o" | "oui" | "y" | "yes"))
            }
        }
    }
}
