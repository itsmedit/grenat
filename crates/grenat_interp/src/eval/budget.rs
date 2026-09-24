//! Budgets de dépense (`within budget(…)`).

use crate::prelude::*;

impl<'p> Interp<'p> {
    // ── Budgets ──────────────────────────────────────────────

    pub(crate) fn within(&mut self, budget: Arc<Budget>, block: &Value<'p>) -> R<'p> {
        budget.restart_clock();
        self.budgets.push(budget);
        let result = self.call_block(block, Vec::new());
        self.budgets.pop();
        result
    }

    pub(crate) fn check_budgets(&self) -> Result<(), Ctrl<'p>> {
        for budget in self.budgets.iter().rev() {
            if let Some(reason) = budget.exceeded() {
                let mut error = ErrorVal::new("BudgetExceeded", format!("budget dépassé : {reason}"));
                error.fields.push(("spent".into(), Value::Money(budget.spent())));
                error.fields.push(("tokens".into(), Value::Int(budget.tokens() as i64)));
                return Err(Ctrl::Raise(Arc::new(error)));
            }
        }
        Ok(())
    }
}
