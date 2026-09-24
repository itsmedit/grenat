//! Expressions, instructions, variables, constantes, affectation, indexation.

use std::collections::HashMap;

use grenat_ast::{Arg, ArmTest, BinOp, Block, Body, Diagnostic, Expr, ExprKind, Ident, Span, StrSeg, TypeKind, UnOp};

use crate::ty::{Ty, V, join, join_v};
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn stmts(&mut self, cx: &mut Ctx<'p>, stmts: &'p [Expr]) -> V {
        let mut last = V::new(Ty::Nil);
        for s in stmts {
            last = self.expr(cx, s);
        }
        last
    }

    pub(crate) fn body(&mut self, cx: &mut Ctx<'p>, body: &'p Body) -> V {
        let mut result = self.stmts(cx, &body.stmts);
        for rescue in &body.rescues {
            if let Some(binding) = &rescue.binding {
                let ty = match rescue.types.as_slice() {
                    [single] => Ty::user(type_name(single)),
                    _ => Ty::user("StandardError"),
                };
                cx.define(&binding.name, V::new(ty));
            }
            for t in &rescue.types {
                let name = type_name(t);
                if !is_error_name(name) && !self.types.contains_key(name) {
                    self.error(E_NAME, t.span(), format!("type d'erreur inconnu `{name}`"));
                }
            }
            let r = self.stmts(cx, &rescue.body);
            result = join_v(&result, &r);
        }
        if let Some(ensure) = &body.ensure {
            self.stmts(cx, ensure);
        }
        result
    }

    pub(crate) fn block(&mut self, cx: &mut Ctx<'p>, block: &'p Block, params: &[V]) -> V {
        cx.scopes.push(HashMap::new());
        let destructure = params.len() == 1 && block.params.len() > 1;
        for (i, p) in block.params.iter().enumerate() {
            let v = if destructure {
                match &params[0].ty {
                    Ty::Array(t) => V { ty: (**t).clone(), taint: params[0].taint },
                    _ => V { ty: Ty::Unknown, taint: params[0].taint },
                }
            } else {
                params.get(i).cloned().unwrap_or_else(V::unknown)
            };
            let v = match &p.ty {
                Some(t) => V { ty: self.resolve(t).0, taint: v.taint },
                None => v,
            };
            cx.define(&p.name.name, v);
        }
        let result = self.body(cx, &block.body);
        cx.scopes.pop();
        result
    }

    pub(crate) fn expr(&mut self, cx: &mut Ctx<'p>, e: &'p Expr) -> V {
        match &e.kind {
            ExprKind::Int(_) => V::new(Ty::Int),
            ExprKind::Float(_) => V::new(Ty::Float),
            ExprKind::Bool(_) => V::new(Ty::Bool),
            ExprKind::Nil => V::new(Ty::Nil),
            ExprKind::Symbol(_) => V::new(Ty::Sym),
            ExprKind::Str(segs) => {
                let mut v = V::new(Ty::Str);
                for seg in segs {
                    if let StrSeg::Interp(e) = seg {
                        let part = self.expr(cx, e);
                        v.taint = v.taint.or(part.taint);
                    }
                }
                v
            }
            ExprKind::SelfRef => match &cx.self_ty {
                Some(ty) => V { ty: ty.clone(), taint: cx.self_taint },
                None => {
                    self.error(E_NAME, e.span, "`self` utilisé hors d'une méthode");
                    V::unknown()
                }
            },
            ExprKind::Array(items) => {
                let mut elem: Option<Ty> = None;
                let mut taint = None;
                for item in items {
                    let v = self.expr(cx, item);
                    taint = taint.or(v.taint);
                    elem = Some(match elem {
                        None => v.ty,
                        Some(t) => join(&t, &v.ty),
                    });
                }
                V { ty: Ty::array(elem.unwrap_or(Ty::Unknown)), taint }
            }
            ExprKind::Hash(entries) => {
                let (mut k, mut val, mut taint) = (None::<Ty>, None::<Ty>, None);
                for (key, value) in entries {
                    let (kv, vv) = (self.expr(cx, key), self.expr(cx, value));
                    taint = taint.or(kv.taint).or(vv.taint);
                    k = Some(k.map_or(kv.ty.clone(), |t| join(&t, &kv.ty)));
                    val = Some(val.map_or(vv.ty.clone(), |t| join(&t, &vv.ty)));
                }
                V { ty: Ty::Hash(Box::new(k.unwrap_or(Ty::Unknown)), Box::new(val.unwrap_or(Ty::Unknown))), taint }
            }
            ExprKind::Range { lo, hi, .. } => {
                for bound in [lo, hi] {
                    let v = self.expr(cx, bound);
                    if !self.compat(&v.ty, &Ty::Int) {
                        self.error(
                            E_TYPE,
                            bound.span,
                            format!("les bornes d'un intervalle sont des `Int`, reçu `{}`", v.ty),
                        );
                    }
                }
                V::new(Ty::Range)
            }
            ExprKind::Var(name) => self.var(cx, name, e.span),
            ExprKind::It => cx.lookup("it").unwrap_or_else(V::unknown),
            ExprKind::Const(path) => self.constant(path, e.span),
            ExprKind::IVar(name) => self.ivar(cx, name, e.span),
            ExprKind::Call { recv, name, args, block, safe, .. } => {
                self.call(cx, e.span, recv.as_deref(), name, args, block.as_deref(), *safe)
            }
            ExprKind::Index { recv, args } => {
                let target = self.expr(cx, recv);
                let index: Vec<V> = args.iter().map(|a| self.expr(cx, a)).collect();
                self.index(target, &index, e.span)
            }
            ExprKind::Unary { op, expr } => {
                let v = self.expr(cx, expr);
                match op {
                    UnOp::Not => V { ty: Ty::Bool, taint: v.taint },
                    UnOp::Neg => {
                        if !matches!(v.ty, Ty::Int | Ty::Float | Ty::Money | Ty::Unknown) {
                            self.error(E_TYPE, e.span, format!("`-` non défini pour `{}`", v.ty));
                        }
                        v
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.expr(cx, lhs), self.expr(cx, rhs));
                self.binary(cx, *op, l, r, e.span, lhs)
            }
            ExprKind::Try(inner) => {
                let v = self.expr(cx, inner);
                let ty = match v.ty {
                    Ty::Result(t, _) => *t,
                    Ty::Opt(t) => *t,
                    other => other,
                };
                V { ty, taint: v.taint }
            }
            ExprKind::Assign { target, value } => {
                let v = self.expr(cx, value);
                self.assign(cx, target, v.clone());
                v
            }
            ExprKind::OpAssign { op, target, value } => {
                let current = match &target.kind {
                    ExprKind::Var(name) => cx.lookup(name).unwrap_or_else(|| V::new(Ty::Nil)),
                    _ => self.expr(cx, target),
                };
                let rhs = self.expr(cx, value);
                let v = match op {
                    BinOp::Or | BinOp::And => join_v(&current, &rhs),
                    op => self.binary(cx, *op, current, rhs, value.span, target),
                };
                self.assign(cx, target, v.clone());
                v
            }
            ExprKind::MultiAssign { targets, value } => {
                let v = self.expr(cx, value);
                let item = match &v.ty {
                    Ty::Array(t) => (**t).clone(),
                    _ => Ty::Unknown,
                };
                for t in targets {
                    self.assign(cx, t, V { ty: item.clone(), taint: v.taint });
                }
                v
            }
            ExprKind::If { cond, then, else_ } => {
                self.expr(cx, cond);
                let a = self.stmts(cx, then);
                let b = match else_ {
                    Some(else_) => self.stmts(cx, else_),
                    None => V::new(Ty::Nil),
                };
                join_v(&a, &b)
            }
            ExprKind::While { cond, body } => {
                self.expr(cx, cond);
                self.stmts(cx, body);
                V::new(Ty::Nil)
            }
            ExprKind::Case { subject, arms, else_ } => {
                let subject = subject.as_ref().map(|s| self.expr(cx, s));
                let mut result: Option<V> = None;
                for arm in arms {
                    match &arm.test {
                        ArmTest::In(pattern) => {
                            let s = subject.clone().unwrap_or_else(V::unknown);
                            self.pattern(cx, pattern, &s);
                        }
                        ArmTest::When(values) => {
                            for v in values {
                                self.expr(cx, v);
                            }
                        }
                    }
                    if let Some(guard) = &arm.guard {
                        self.expr(cx, guard);
                    }
                    let v = self.stmts(cx, &arm.body);
                    result = Some(result.map_or(v.clone(), |r| join_v(&r, &v)));
                }
                if let Some(else_) = else_ {
                    let v = self.stmts(cx, else_);
                    result = Some(result.map_or(v.clone(), |r| join_v(&r, &v)));
                }
                result.unwrap_or_else(|| V::new(Ty::Nil))
            }
            ExprKind::Begin(body) => self.body(cx, body),
            ExprKind::Return(value) => {
                let v = value.as_ref().map_or(V::new(Ty::Nil), |v| self.expr(cx, v));
                cx.returns.push(v);
                V::unknown()
            }
            ExprKind::Break(value) | ExprKind::Next(value) => {
                if let Some(v) = value {
                    self.expr(cx, v);
                }
                V::unknown()
            }
        }
    }

    pub(crate) fn var(&mut self, cx: &mut Ctx<'p>, name: &str, span: Span) -> V {
        if let Some(v) = cx.lookup(name) {
            return v;
        }
        if let Some(self_ty) = cx.self_ty.clone() {
            if let Some(field) = self.field_of(&self_ty, name) {
                return V { ty: field, taint: cx.self_taint };
            }
            if let Some(def) = self.method_def(&self_ty, name) {
                let recv = V { ty: self_ty, taint: cx.self_taint };
                return self.user_method(cx, span, recv, def, Vec::new(), None);
            }
        }
        if let Some(def) = self.fns.get(name).copied() {
            return self.fn_call(cx, span, def, None, Vec::new(), None);
        }
        match name {
            "budget" => return V::new(Ty::Budget),
            "deny_all" | "approve_all" => return V::new(Ty::Sym),
            "puts" | "print" | "p" | "warn" => return V::new(Ty::Nil),
            _ => {}
        }
        if let Some(base) = name.strip_suffix('?')
            && let Some(v) = cx.lookup(base)
        {
            // `r?` : opérateur `?` sur une variable
            let ty = match v.ty {
                Ty::Result(t, _) | Ty::Opt(t) => *t,
                other => other,
            };
            return V { ty, taint: v.taint };
        }
        let mut candidates = cx.names();
        candidates.extend(self.fns.keys().map(|s| s.to_string()));
        self.error_help(
            E_NAME,
            span,
            format!("variable ou fonction inconnue `{name}`"),
            suggest(name, candidates.iter().map(String::as_str)),
        );
        V::unknown()
    }

    pub(crate) fn constant(&mut self, path: &'p [Ident], span: Span) -> V {
        let name = path.last().expect("chemin").name.as_str();
        if path.len() >= 2 {
            let owner = path[path.len() - 2].name.as_str();
            if self.variants.get(name) == Some(&owner) {
                return self.variant_value(name);
            }
        }
        if self.types.contains_key(name)
            || builtins::MODULES.contains(&name)
            || builtins::TYPE_NAMES.contains(&name)
            || is_error_name(name)
            || self.messages.contains_key(name)
        {
            return V::new(Ty::Type(name.into()));
        }
        if self.variants.contains_key(name) {
            return self.variant_value(name);
        }
        let known: Vec<&str> =
            self.types.keys().chain(self.variants.keys()).copied().chain(builtins::MODULES.iter().copied()).collect();
        self.error_help(E_NAME, span, format!("constante inconnue `{name}`"), suggest(name, known));
        V::unknown()
    }

    pub(crate) fn variant_value(&self, name: &str) -> V {
        let enum_name = self.variants[name];
        let has_fields = self.types[enum_name].variants.iter().any(|v| v.name.name == name && !v.fields.is_empty());
        V::new(if has_fields { Ty::Type(name.into()) } else { Ty::user(enum_name) })
    }

    pub(crate) fn ivar(&mut self, cx: &Ctx<'p>, name: &str, span: Span) -> V {
        let Some(Ty::User(owner)) = &cx.self_ty else {
            self.error(E_NAME, span, format!("`@{name}` utilisé hors d'une classe ou d'un agent"));
            return V::unknown();
        };
        let Some(field) =
            self.types.get(owner.as_str()).and_then(|t| t.ivars.iter().find(|f| f.name.name == name).copied())
        else {
            let known: Vec<&str> = self
                .types
                .get(owner.as_str())
                .map(|t| t.ivars.iter().map(|f| f.name.name.as_str()).collect())
                .unwrap_or_default();
            self.error_help(
                E_NAME,
                span,
                format!("état `@{name}` non déclaré dans `{owner}`"),
                suggest(name, known).or_else(|| Some(format!("déclarez-le : `@{name}: Type = valeur`"))),
            );
            return V::unknown();
        };
        let ty = match &field.ty {
            Some(t) => self.peek_ty(t),
            None => self.ivar_types.get(&(owner.clone(), name.to_string())).cloned().unwrap_or(Ty::Unknown),
        };
        let taint = self.ivar_taint.get(&(owner.clone(), name.to_string())).copied();
        V { ty, taint }
    }

    pub(crate) fn assign(&mut self, cx: &mut Ctx<'p>, target: &'p Expr, value: V) {
        match &target.kind {
            ExprKind::Var(name) => cx.assign(name, value),
            ExprKind::IVar(name) => {
                let current = self.ivar(cx, name, target.span);
                if !self.compat(&value.ty, &current.ty) {
                    self.error(
                        E_TYPE,
                        target.span,
                        format!("`@{name}` est de type `{}`, reçu `{}`", current.ty, value.ty),
                    );
                }
                if let (Some(origin), Some(Ty::User(owner))) = (value.taint, &cx.self_ty) {
                    self.ivar_taint.entry((owner.clone(), name.clone())).or_insert(origin);
                }
            }
            ExprKind::Index { recv, args } => {
                self.expr(cx, recv);
                for a in args {
                    self.expr(cx, a);
                }
                self.taint_container(cx, recv, value.taint);
            }
            ExprKind::Call { recv: Some(recv), name, .. } => {
                let r = self.expr(cx, recv);
                if let Ty::User(t) = &r.ty
                    && self.types.get(t.as_str()).is_some_and(|d| d.def.kind == TypeKind::Struct)
                {
                    self.report(
                        Diagnostic::new(target.span, format!("`{t}` est une struct immuable"))
                            .with_code(E_TYPE)
                            .with_help(format!("créez une copie : `.with({}: …)`", name.name)),
                    );
                }
            }
            _ => {}
        }
    }

    pub(crate) fn index(&mut self, target: V, index: &[V], span: Span) -> V {
        let key = index.first().map_or(Ty::Unknown, |v| v.ty.clone());
        let ty = match (target.ty.base(), &key) {
            (Ty::Array(t), Ty::Range) => Ty::Array(t.clone()),
            (Ty::Array(t), _) => (**t).clone(),
            (Ty::Hash(_, v), _) => (**v).clone(),
            (Ty::Str, _) => Ty::Str,
            (Ty::Type(sup), Ty::Type(agent)) => {
                let declared = self.types.get(sup.as_str()).is_some_and(|t| {
                    t.def.kind == TypeKind::Supervisor
                        && t.directives.iter().any(|d| {
                            d.name.name == "child"
                                && matches!(d.args.first(), Some(Arg::Pos(Expr { kind: ExprKind::Const(p), .. }))
                                    if p.last().is_some_and(|i| &i.name == agent))
                        })
                });
                if !declared {
                    self.error(E_NAME, span, format!("`{agent}` n'est pas un enfant du superviseur `{sup}`"));
                }
                Ty::user(agent)
            }
            // indice déjà signalé (constante inconnue…) : pas d'erreur en cascade
            (Ty::Unknown, _) | (_, Ty::Unknown) => Ty::Unknown,
            (Ty::Type(name), _) => {
                self.error(E_TYPE, span, format!("`{name}` ne peut pas être indexé"));
                Ty::Unknown
            }
            (other, _) => {
                self.error(E_TYPE, span, format!("`{other}` ne peut pas être indexé"));
                Ty::Unknown
            }
        };
        V { ty, taint: target.taint }
    }
}
