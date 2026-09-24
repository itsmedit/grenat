//! The `~T` taint: tainted declarations, dangerous sinks, propagation into containers.

use grenat_ast::{Diagnostic, Expr, ExprKind, FnDef, Param, Span, Type};

use crate::ty::Ty;
use crate::*;

pub(crate) const DANGEROUS_EFFECTS: &[&str] = &["shell", "net", "fs.write", "human"];

pub(crate) const TAINT_HELP: &str = "validate it with `.check { … }`, `.approve(by: :human)` or `.trust!`";

pub(crate) fn is_tainted_decl(ty: &Type) -> bool {
    match ty {
        Type::Tainted(..) => true,
        Type::Optional(inner, _) => is_tainted_decl(inner),
        Type::Named { .. } => false,
    }
}

pub(crate) fn declared_taints(params: &[Param]) -> Vec<Option<Span>> {
    params.iter().map(|p| p.ty.as_ref().filter(|t| is_tainted_decl(t)).map(|_| p.span)).collect()
}

pub(crate) fn dangerous_effect(def: &FnDef) -> Option<String> {
    def.effects.iter().find_map(|e| {
        let path = e.path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(".");
        DANGEROUS_EFFECTS.contains(&path.as_str()).then_some(path)
    })
}

impl<'p> Checker<'p> {
    /// A tainted value reaching a dangerous effect: E0412.
    pub(crate) fn taint_violation(&mut self, arg_span: Span, origin: Span, target: &str, effect: &str) {
        self.report(
            Diagnostic::new(
                arg_span,
                format!("an LLM-produced value reaches `{target}` (effect `{effect}`) without validation"),
            )
            .with_code(E_TAINT)
            .with_note(origin, "produced here by an LLM")
            .with_help(TAINT_HELP),
        );
    }

    /// `xs << v`, `h[k] = v`: the container becomes tainted if `v` is.
    pub(crate) fn taint_container(&mut self, cx: &mut Ctx<'p>, container: &'p Expr, taint: Option<Span>) {
        let Some(origin) = taint else { return };
        match &container.kind {
            ExprKind::Var(name) => {
                if let Some(v) = cx.lookup(name) {
                    cx.assign(name, v.tainted(Some(origin)));
                }
            }
            ExprKind::IVar(name) => {
                if let Some(Ty::User(owner)) = &cx.self_ty {
                    self.ivar_taint.entry((owner.clone(), name.clone())).or_insert(origin);
                }
            }
            _ => {}
        }
    }
}
