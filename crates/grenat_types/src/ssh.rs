//! The methods of an SSH connection and of its SFTP files: an `ssh` effect
//! (on the connection's host, checked at run time), and for a transfer the
//! local file's `fs.read` (`upload`) or `fs.write` (`download`). Nothing
//! untrusted reaches the server (E0412), no secret either (E0414); what is
//! read from it is untrusted.

use grenat_ast::Span;

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn ssh_method(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        (ty, effect): (Ty, &'static str),
        (record, name): (&str, &str),
        argv: &[ArgV],
    ) -> V {
        cx.add_effect(Eff { path: effect.into(), arg: None, origin: span });
        if let Some((index, local)) = builtins::local_side(record, name) {
            let path = argv.iter().filter(|a| a.name.is_none()).nth(index).and_then(|a| a.lit.clone());
            cx.add_effect(Eff { path: local.into(), arg: path, origin: span });
        }
        for arg in argv {
            if let Some(origin) = arg.v.taint {
                self.taint_violation(arg.span, origin, name, effect);
            }
            if self.holds_secret(&arg.v.ty, arg.span) {
                self.secret_to_server(arg.span, name);
            }
        }
        V { ty, taint: builtins::untrusted_method(record, name).then_some(span) }
    }
}
