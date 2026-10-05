//! Test doubles that exist inside a `test` block only, checked:
//! `mock_env({"NAME" => "value"})` (the environment `Env` reads),
//! `mock_mail(raise: "SMTP down")` (sending fails) and `Http.requests`
//! (what the test sent); `freeze_time` (see `clock`) shares the rule.

use grenat_ast::{Diagnostic, Span};

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// Reports `what` outside a test (E0500): a program never runs with it.
    pub(crate) fn only_in_tests(&mut self, cx: &Ctx<'p>, span: Span, what: &str, help: &str) {
        if cx.tests == 0 {
            self.report(
                Diagnostic::new(span, format!("`{what}` can only be used inside a test (`test \"…\" do … end`)"))
                    .with_code(E_DECL)
                    .with_help(help.to_string()),
            );
        }
    }

    /// `mock_env({"GITHUB_TOKEN" => "t"})`: a hash of strings.
    pub(crate) fn mock_env_call(&mut self, cx: &Ctx<'p>, span: Span, argv: &[ArgV]) -> V {
        self.only_in_tests(cx, span, "mock_env", "outside tests, `Env` reads the process's environment");
        let variables = Ty::Hash(Box::new(Ty::Str), Box::new(Ty::Str));
        match argv {
            [arg] if arg.name.is_none() => {
                if secrets::is_secret(&arg.v.ty) || !self.compat(&arg.v.ty, &variables) {
                    let message = format!("`mock_env` expects `Hash(String, String)`, got `{}`", arg.v.ty);
                    self.error(E_TYPE, arg.span, message);
                }
            }
            _ => self.error(E_TYPE, span, "`mock_env` expects a hash: `mock_env({\"GITHUB_TOKEN\" => \"t\"})`"),
        }
        V::new(Ty::Nil)
    }

    /// `mock_mail(raise: "SMTP down")` (`raise: nil`: sending works again).
    pub(crate) fn mock_mail_call(&mut self, cx: &Ctx<'p>, span: Span, argv: &[ArgV]) -> V {
        self.only_in_tests(cx, span, "mock_mail", "it makes a test's emails fail");
        match argv {
            [arg] if arg.name.as_deref() == Some("raise") => {
                if !self.compat(&arg.v.ty, &Ty::opt(Ty::Str)) {
                    let message = format!("`mock_mail` expects `raise:` a message (`String`), got `{}`", arg.v.ty);
                    self.error(E_TYPE, arg.span, message);
                }
            }
            _ => self.error(E_TYPE, span, "`mock_mail` expects why sending fails: `mock_mail(raise: \"SMTP down\")`"),
        }
        V::new(Ty::Nil)
    }
}
