//! Converts the internal outcome into a public error or summary.

use std::sync::atomic::Ordering;

use crate::value::Locked;
use crate::*;

impl<'p> Interp<'p> {
    pub(crate) fn runtime_error(&self, ctrl: Ctrl<'p>) -> RuntimeError {
        match ctrl {
            Ctrl::Raise(e) => RuntimeError {
                ty: e.ty.to_string(),
                message: e.message.clone(),
                span: e.span(),
                trace: e.trace.borrow().clone(),
            },
            Ctrl::Break(_) | Ctrl::Next(_) => RuntimeError {
                ty: "LocalJumpError".into(),
                message: "`break` or `next` outside a loop or block".into(),
                span: None,
                trace: Vec::new(),
            },
            Ctrl::Return(_) | Ctrl::Exit(_) => unreachable!("handled by the caller"),
        }
    }

    pub(crate) fn summary(&self, exit_code: i32) -> Summary {
        let total = &self.total;
        Summary {
            exit_code,
            llm_calls: self.llm_calls.load(Ordering::Relaxed),
            tokens: total.tokens(),
            cost_usd: total.spent(),
        }
    }
}
