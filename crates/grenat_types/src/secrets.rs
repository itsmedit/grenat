//! Secrets (`Credentials.fetch`): a `Secret` is not a `String` — it goes
//! where it serves (HTTP URLs and headers, connections), never to a model.
//! What the checker proves here, the runtime enforces too.

use grenat_ast::Span;

use crate::ty::{Ty, V};
use crate::*;

const HELP: &str = "a secret serves in HTTP URLs and headers, and in connections (`Db.connect`, `Mail.connect`, `mcp`); a function that takes one declares `Secret`";

impl<'p> Checker<'p> {
    /// Reports the secrets among `argv`: they would reach a model through `target`.
    pub(crate) fn secrets_to_model(&mut self, argv: &[ArgV], target: &str) {
        for arg in argv.iter().filter(|a| is_secret(&a.v.ty)) {
            self.report(
                Diagnostic::new(arg.span, format!("a secret reaches a model through `{target}`"))
                    .with_code(E_SECRET)
                    .with_help(HELP),
            );
        }
    }

    /// A secret given where a `String` is expected.
    pub(crate) fn secret_as_string(&mut self, span: Span, owner: &str, slot: &str) {
        self.report(
            Diagnostic::new(span, format!("`{owner}` expects a `String` for `{slot}`: a secret is not one"))
                .with_code(E_SECRET)
                .with_help(HELP),
        );
    }

    /// A method of a secret: only `to_s` (still a secret).
    pub(crate) fn secret_method(&mut self, span: Span, name: &str) -> V {
        if name != "to_s" {
            self.report(
                Diagnostic::new(span, format!("a secret has no method `{name}`"))
                    .with_code(E_SECRET)
                    .with_help(HELP),
            );
        }
        V::new(Ty::Secret)
    }
}

pub(crate) fn is_secret(ty: &Ty) -> bool {
    matches!(ty.base(), Ty::Secret)
}
