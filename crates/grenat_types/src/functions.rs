//! Checking functions and handlers for a given taint of their arguments.

use grenat_ast::{FnDef, FnKind, Handler, Param, Span};

use crate::ty::{Ty, V, join_v};
use crate::*;

impl<'p> Checker<'p> {
    /// Checks `def` for a given taint of its arguments (context-sensitive analysis).
    pub(crate) fn check_fn(
        &mut self,
        def: &'p FnDef,
        self_ty: Option<Ty>,
        taints: Vec<Option<Span>>,
        self_taint: Option<Span>,
    ) -> (V, Vec<Eff>) {
        let key: Key =
            (def as *const FnDef as usize, taints.iter().map(Option::is_some).collect(), self_taint.is_some());
        if let Some(result) = self.memo.get(&key) {
            return result.clone();
        }
        let declared_ret = def.ret.as_ref().map(|t| self.resolve(t));
        if self.in_progress.contains(&key) {
            let ty = declared_ret.map_or(Ty::Unknown, |r| r.0);
            let taint = taints.iter().flatten().next().copied().or(self_taint);
            return (V { ty, taint }, self.prev_effects.get(&key.0).cloned().unwrap_or_default());
        }
        self.in_progress.insert(key.clone());

        let mut cx = Ctx::new(Kind::Fn(def), self_ty, self_taint);
        self.bind_params(&mut cx, &def.params, &taints);
        let body = self.body(&mut cx, &def.body);
        let mut ret = cx.returns.iter().fold(body, |acc, r| join_v(&acc, r));

        match (&declared_ret, def.kind) {
            (_, FnKind::Prompt) => {
                ret = V { ty: declared_ret.map_or(Ty::Str, |r| r.0), taint: Some(def.span) };
            }
            (Some((ty, tainted)), _) => {
                let checkable = !def.is_abstract && *ty != Ty::Nil;
                if checkable && !self.compat(&ret.ty, ty) {
                    let span = def.body.stmts.last().map_or(def.span, |e| e.span);
                    self.error(E_TYPE, span, format!("`{}` must return `{ty}`, returns `{}`", def.name.name, ret.ty));
                }
                ret.ty = ty.clone();
                if *tainted {
                    ret.taint = ret.taint.or(Some(def.ret.as_ref().expect("declared").span()));
                }
            }
            (None, _) => {}
        }

        let effects = self.finish_effects(def, &cx);
        self.in_progress.remove(&key);
        self.prev_effects.insert(key.0, effects.clone());
        self.memo.insert(key, (ret.clone(), effects.clone()));
        (ret, effects)
    }

    pub(crate) fn bind_params(&mut self, cx: &mut Ctx<'p>, params: &'p [Param], taints: &[Option<Span>]) {
        for (i, p) in params.iter().enumerate() {
            let mut v = match &p.ty {
                Some(t) => {
                    let (ty, tainted) = self.resolve(t);
                    V { ty, taint: tainted.then_some(p.span) }
                }
                None => V::unknown(),
            };
            if let Some(default) = &p.default {
                let d = self.expr(cx, default);
                if p.ty.is_none() {
                    v.ty = d.ty;
                }
            }
            v.taint = v.taint.or(taints.get(i).copied().flatten());
            cx.define(&p.name.name, v);
        }
    }

    pub(crate) fn check_handler(
        &mut self,
        agent: &'p str,
        handler: &'p Handler,
        taints: Vec<Option<Span>>,
    ) -> (V, Vec<Eff>) {
        let key: Key = (handler as *const Handler as usize, taints.iter().map(Option::is_some).collect(), false);
        if let Some(result) = self.memo.get(&key) {
            return result.clone();
        }
        let declared_ret = handler.ret.as_ref().map(|t| self.resolve(t));
        if self.in_progress.contains(&key) {
            let ty = declared_ret.map_or(Ty::Unknown, |r| r.0);
            return (
                V { ty, taint: taints.iter().flatten().next().copied() },
                self.prev_effects.get(&key.0).cloned().unwrap_or_default(),
            );
        }
        self.in_progress.insert(key.clone());

        let mut cx = Ctx::new(Kind::Handler(agent, handler), Some(Ty::user(agent)), None);
        self.bind_params(&mut cx, &handler.params, &taints);
        let body = self.body(&mut cx, &handler.body);
        let mut ret = cx.returns.iter().fold(body, |acc, r| join_v(&acc, r));
        if let Some((ty, tainted)) = &declared_ret {
            if cx.run_span.is_none() && *ty != Ty::Nil && !self.compat(&ret.ty, ty) {
                let span = handler.body.stmts.last().map_or(handler.span, |e| e.span);
                self.error(
                    E_TYPE,
                    span,
                    format!("`on {}` must return `{ty}`, returns `{}`", handler.message.name, ret.ty),
                );
            }
            ret.ty = ty.clone();
            if *tainted {
                ret.taint = ret.taint.or(cx.run_span).or(Some(handler.span));
            }
        }
        let effects = cx.effects.clone();
        self.in_progress.remove(&key);
        self.prev_effects.insert(key.0, effects.clone());
        self.memo.insert(key, (ret.clone(), effects.clone()));
        (ret, effects)
    }
}
