//! `format("%.2f", x)` and `"%05d" % n`: a `String` — a `Secret` when a
//! secret is written in it, as interpolation makes one. What is made of an
//! untrusted value is untrusted.

use grenat_ast::Span;

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// `format(spec, values…)`.
    pub(crate) fn format_call(&mut self, span: Span, argv: &[ArgV]) -> V {
        let Some((spec, values)) = argv.split_first() else {
            self.error(E_TYPE, span, "`format` expects a format string: `format(\"%.2f\", x)`");
            return V::new(Ty::Str);
        };
        if !self.compat(&spec.v.ty, &Ty::Str) {
            self.error(E_TYPE, spec.span, format!("`format` expects a `String` format, got `{}`", spec.v.ty));
        }
        let secret = values.iter().any(|a| self.holds_secret(&a.v.ty, a.span));
        let taint = argv.iter().find_map(|a| a.v.taint);
        V { ty: formatted(secret), taint }
    }
}

/// What formatting makes: a secret written in it makes a secret.
pub(crate) fn formatted(secret: bool) -> Ty {
    if secret { Ty::Secret } else { Ty::Str }
}
