//! Options of `Http` and of its test double, checked as the runtime checks
//! them: `retry:` true or false (`Http.post(url, retry: true)`), and
//! `mock_http`'s `times:` a positive count (`times: 2`; `0`, `-1` and
//! `"2"` are refused when the test runs, so here too).

use crate::ty::Ty;
use crate::*;

impl<'p> Checker<'p> {
    /// `Http.get(url, retry: false)`: `retry:` a `Bool`.
    pub(crate) fn http_options(&mut self, argv: &[ArgV]) {
        for arg in argv.iter().filter(|a| a.name.as_deref() == Some("retry")) {
            if !self.compat(&arg.v.ty, &Ty::Bool) {
                let message = format!("`Http` expects `retry:` true or false, got `{}`", arg.v.ty);
                self.error(E_TYPE, arg.span, message);
            }
        }
    }

    /// `mock_http "GET https://…", status: 429, times: 2`: `times:` a positive `Int`.
    pub(crate) fn mock_http_options(&mut self, argv: &[ArgV]) {
        for arg in argv.iter().filter(|a| a.name.as_deref() == Some("times")) {
            let literal_bad = arg.number.is_some_and(|n| n < 1.0 || n.fract() != 0.0);
            if !self.compat(&arg.v.ty, &Ty::Int) || literal_bad {
                let message = format!("`mock_http` expects `times:` a positive count, got `{}`", arg.v.ty);
                self.error(E_TYPE, arg.span, message);
            }
        }
    }
}
