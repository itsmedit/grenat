//! Secrets (`Credentials.fetch`): a `Secret` is not a `String` — it goes
//! where it serves (HTTP URLs and headers, connections), never to a model.
//! What the checker proves here, the runtime enforces too.

use grenat_ast::Span;

use crate::ty::{Ty, V};
use crate::*;

const HELP: &str = "a secret serves in HTTP URLs and headers, and in connections (`Db.connect`, `Mail.connect`, `mcp`, `Ssh.connect`); a function that takes one declares `Secret`";

impl<'p> Checker<'p> {
    /// Reports the secrets among `argv`: they would reach a model through `target`.
    pub(crate) fn secrets_to_model(&mut self, argv: &[ArgV], target: &str) {
        // a secret, or a literal holding one (`["x", token]`)
        let secrets: Vec<Span> = argv.iter().filter(|a| self.holds_secret(&a.v.ty, a.span)).map(|a| a.span).collect();
        for span in secrets {
            self.report(
                Diagnostic::new(span, format!("a secret reaches a model through `{target}`"))
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

    /// A secret given to an SSH server through `target` (a command, a path,
    /// a file's content): it only serves to connect.
    pub(crate) fn secret_to_server(&mut self, span: Span, target: &str) {
        self.report(
            Diagnostic::new(span, format!("a secret reaches an SSH server through `{target}`"))
                .with_code(E_SECRET)
                .with_help(
                    "a secret serves to connect: `key:`, `passphrase:`, `password:` and `proxy:` of `Ssh.connect`",
                ),
        );
    }

    /// A secret sent to a client in a streamed response.
    pub(crate) fn secret_to_client(&mut self, span: Span) {
        self.report(
            Diagnostic::new(span, "a secret is sent to a client through a streamed response")
                .with_code(E_SECRET)
                .with_help(HELP),
        );
    }

    /// A method of a secret: only `to_s` (still a secret).
    pub(crate) fn secret_method(&mut self, span: Span, name: &str) -> V {
        if name != "to_s" {
            self.report(
                Diagnostic::new(span, format!("a secret has no method `{name}`")).with_code(E_SECRET).with_help(HELP),
            );
        }
        V::new(Ty::Secret)
    }

    /// Whether the value of type `ty` given by the expression at `span`
    /// holds a secret: a secret (maybe nil), a collection of secrets, or an
    /// array or hash literal with one among its elements.
    pub(crate) fn holds_secret(&self, ty: &Ty, span: Span) -> bool {
        type_holds_secret(ty) || self.secret_literals.contains(&span)
    }

    /// Records that the literal at `span` holds a secret, when `secret`.
    pub(crate) fn note_secret_literal(&mut self, span: Span, secret: bool) {
        if secret {
            self.secret_literals.insert(span);
        }
    }
}

pub(crate) fn is_secret(ty: &Ty) -> bool {
    matches!(ty.base(), Ty::Secret)
}

/// A secret, or a collection holding secrets.
pub(crate) fn type_holds_secret(ty: &Ty) -> bool {
    match ty.base() {
        Ty::Secret => true,
        Ty::Array(item) => type_holds_secret(item),
        Ty::Hash(key, value) => type_holds_secret(key) || type_holds_secret(value),
        _ => false,
    }
}
