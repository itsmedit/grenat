//! `native def`: functions a native facet implements in Rust. Their
//! declarations are all the checker sees — types that cross the boundary
//! as JSON, declared effects (which callers must cover, as for any call),
//! a result untrusted (`~T`) unless the function is `pure` — and no secret
//! is handed to them.

use grenat_ast::{Diagnostic, FnDef, Span, TypeKind};

use crate::ty::{Ty, V};
use crate::*;

/// The types that cross to native code, for the help of a refusal.
const CROSSING: &str =
    "`Int`, `Float`, `String`, `Bool`, `Array(T)`, `Hash(String, T)`, `T?` and structs of those cross to native code";

impl<'p> Checker<'p> {
    /// A `native def`'s declaration: its types, its purity, its result's taint.
    pub(crate) fn check_native_decl(&mut self, def: &'p FnDef) {
        for param in &def.params {
            match &param.ty {
                None => {
                    self.error(E_DECL, param.span, format!("native parameter `{}` must have a type", param.name.name))
                }
                Some(t) => {
                    let ty = self.resolve(t).0;
                    if let Err(e) = self.crosses(&ty, 0) {
                        self.report(Diagnostic::new(t.span(), e).with_code(E_DECL).with_help(CROSSING));
                    }
                }
            }
        }
        if def.pure && !def.effects.is_empty() {
            self.report(
                Diagnostic::new(def.effects[0].span, format!("`{}` is `pure`: it has no effects", def.name.name))
                    .with_code(E_DECL)
                    .with_help("a pure function's result depends on its arguments only: drop `pure` or `uses`"),
            );
        }
        let Some(ret) = &def.ret else { return };
        let ty = self.resolve(ret).0;
        if ty != Ty::Nil
            && let Err(e) = self.crosses(&ty, 0)
        {
            self.report(Diagnostic::new(ret.span(), e).with_code(E_DECL).with_help(CROSSING));
        }
        match (def.pure, is_tainted_decl(ret)) {
            (true, true) => self.report(
                Diagnostic::new(ret.span(), format!("`{}` is `pure`: its result is trusted", def.name.name))
                    .with_code(E_TAINT_DECL)
                    .with_help(format!("write `-> {}`", type_name(ret))),
            ),
            (false, false) if ty != Ty::Nil => self.report(
                Diagnostic::new(
                    ret.span(),
                    "a native function's result comes from outside Grenat: its type must be tainted",
                )
                .with_code(E_TAINT_DECL)
                .with_help(format!("write `-> ~{}`, or declare the function `pure`", type_name(ret))),
            ),
            _ => {}
        }
    }

    /// Whether values of `ty` cross to native code (as JSON), and why not.
    fn crosses(&self, ty: &Ty, depth: usize) -> Result<(), String> {
        if depth > 16 {
            return Err("a recursive type cannot cross to native code".into());
        }
        match ty {
            Ty::Unknown | Ty::Int | Ty::Float | Ty::Str | Ty::Bool => Ok(()),
            Ty::Array(t) | Ty::Opt(t) => self.crosses(t, depth + 1),
            Ty::Hash(key, value) if matches!(**key, Ty::Str | Ty::Unknown) => self.crosses(value, depth + 1),
            Ty::Hash(..) => Err("a hash crosses to native code with `String` keys: `Hash(String, T)`".into()),
            Ty::Secret => Err("a secret never reaches native code".into()),
            Ty::User(name) => match self.types.get(name.as_str()) {
                Some(decl) if decl.def.kind == TypeKind::Struct => {
                    for field in &decl.fields {
                        let Some(t) = &field.ty else { return Err(format!("field `{}` has no type", field.name.name)) };
                        self.crosses(&self.peek_ty(t), depth + 1)?;
                    }
                    Ok(())
                }
                _ => Err(format!("`{name}` cannot cross to native code: only structs do")),
            },
            other => Err(format!("`{other}` cannot cross to native code")),
        }
    }

    /// The result of a `native def` checked with these argument taints:
    /// untrusted unless `pure`; a pure one's is as trusted as its arguments.
    pub(crate) fn native_result(&mut self, def: &'p FnDef, taints: &[Option<Span>], self_taint: Option<Span>) -> V {
        let ty = def.ret.as_ref().map_or(Ty::Nil, |t| self.resolve(t).0);
        let taint = if def.pure {
            taints.iter().flatten().next().copied().or(self_taint)
        } else if ty == Ty::Nil {
            None
        } else {
            Some(def.span)
        };
        V { ty, taint }
    }

    /// Secrets among the arguments of a native function: refused (E0414).
    pub(crate) fn secrets_to_native(&mut self, argv: &[ArgV], target: &str) {
        let secrets: Vec<Span> = argv.iter().filter(|a| self.holds_secret(&a.v.ty, a.span)).map(|a| a.span).collect();
        for span in secrets {
            self.report(
                Diagnostic::new(span, format!("a secret reaches native code through `{target}`"))
                    .with_code(E_SECRET)
                    .with_help("native code runs outside Grenat's sandbox: a secret is never handed to it"),
            );
        }
    }
}
