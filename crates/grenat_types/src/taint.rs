//! Teinte `~T` : déclarations teintées, puits dangereux, propagation dans les conteneurs.

use grenat_ast::{Diagnostic, Expr, ExprKind, FnDef, Param, Span, Type};

use crate::ty::Ty;
use crate::*;

pub(crate) const DANGEROUS_EFFECTS: &[&str] = &["shell", "net", "fs.write", "human"];

pub(crate) const TAINT_HELP: &str = "validez-la avec `.check { … }`, `.approve(by: :human)` ou `.trust!`";

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
    /// Une valeur teintée qui atteint un effet dangereux : E0412.
    pub(crate) fn taint_violation(&mut self, arg_span: Span, origin: Span, target: &str, effect: &str) {
        self.report(
            Diagnostic::new(
                arg_span,
                format!("une valeur produite par un LLM atteint `{target}` (effet `{effect}`) sans validation"),
            )
            .with_code(E_TAINT)
            .with_note(origin, "produite ici par un LLM")
            .with_help(TAINT_HELP),
        );
    }

    /// `xs << v`, `h[k] = v` : le conteneur devient teinté si `v` l'est.
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
