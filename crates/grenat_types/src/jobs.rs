//! Jobs, checked: `enqueue(:weekly, "2026-09-28")` or
//! `enqueue(:weekly, monday: "2026-09-28", in: 1.hour)` queues a call, which
//! a worker makes later. A literal function name must name a function of the
//! program (E0100), and the arguments after it — positional and keyword,
//! `in:` (when to run) aside — must bind to its parameters as a call's do
//! (E0200), since the worker calls it with them. A name computed at run time
//! is checked when the job is queued.

use grenat_ast::Span;

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// `enqueue(:name, args…)`: a `db.write` effect; its arguments are
    /// written, never untrusted (E0412), and fit the function.
    pub(crate) fn enqueue_call(&mut self, cx: &mut Ctx<'p>, span: Span, argv: &[ArgV]) -> V {
        cx.add_effect(Eff { path: "db.write".into(), arg: None, origin: span });
        for arg in argv {
            if let Some(origin) = arg.v.taint {
                self.taint_violation(arg.span, origin, "enqueue", "db.write");
            }
        }
        let Some(first) = argv.first().filter(|a| a.name.is_none()) else { return V::new(Ty::Nil) };
        let Some(name) = first.symbol.as_deref() else { return V::new(Ty::Nil) };
        match self.fns.get(name).copied() {
            Some(def) => {
                let args: Vec<ArgV> = argv[1..].iter().filter(|a| a.name.as_deref() != Some("in")).cloned().collect();
                self.bind_args(name, &Slot::params(&def.params), &args, span);
            }
            None => {
                let help = suggest(name, self.fns.keys().copied());
                self.error_help(E_NAME, first.span, format!("`enqueue`: unknown function `{name}`"), help);
            }
        }
        V::new(Ty::Nil)
    }
}
