//! Function calls: arguments, parameter binding, built-in functions.

use grenat_ast::{Arg, Block, Diagnostic, Expr, FnDef, FnKind, Ident, Span, TypeKind};

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn args(&mut self, cx: &mut Ctx<'p>, args: &'p [Arg]) -> Vec<ArgV> {
        let mut out = Vec::new();
        for arg in args {
            match arg {
                Arg::Pos(e) => {
                    let v = self.expr(cx, e);
                    out.push(ArgV { name: None, v, span: e.span, lit: literal_string(e) });
                }
                Arg::Named { name, value } => {
                    let (v, span, lit) = match value {
                        Some(e) => (self.expr(cx, e), e.span, literal_string(e)),
                        None => (self.var(cx, &name.name, name.span), name.span, None),
                    };
                    out.push(ArgV { name: Some(name.name.clone()), v, span, lit });
                }
                Arg::BlockPass(e) => {
                    self.expr(cx, e);
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn call(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        recv: Option<&'p Expr>,
        name: &'p Ident,
        args: &'p [Arg],
        block: Option<&'p Block>,
        safe: bool,
    ) -> V {
        if recv.is_none()
            && name.name == "race"
            && let Some(b) = block
        {
            return self.block(cx, b, &[]);
        }
        let receiver = recv.map(|r| self.expr(cx, r));
        let argv = self.args(cx, args);
        // `xs.push(v)`: the array becomes tainted if `v` is
        if let Some(r) = recv
            && matches!(name.name.as_str(), "push" | "append" | "unshift")
        {
            let taint = argv.iter().find_map(|a| a.v.taint);
            self.taint_container(cx, r, taint);
        }
        let result = match receiver {
            Some(r) => self.method(cx, span, r, name, argv, block),
            None => self.function(cx, span, name, argv, block),
        };
        if safe { V { ty: Ty::opt(result.ty), taint: result.taint } } else { result }
    }

    pub(crate) fn walk_block(&mut self, cx: &mut Ctx<'p>, block: Option<&'p Block>, params: &[V]) -> Option<V> {
        block.map(|b| self.block(cx, b, params))
    }

    pub(crate) fn function(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        name: &'p Ident,
        argv: Vec<ArgV>,
        block: Option<&'p Block>,
    ) -> V {
        let n = name.name.as_str();
        if n.starts_with(|c: char| c.is_uppercase()) {
            self.walk_block(cx, block, &[]);
            return self.construct(span, n, argv);
        }
        if n == "run"
            && let Kind::Handler(agent, handler) = cx.kind
        {
            return self.run(cx, span, agent, handler, argv);
        }
        if matches!(n, "system" | "user" | "assistant") && matches!(cx.kind, Kind::Fn(f) if f.kind == FnKind::Prompt) {
            return V::new(Ty::Nil);
        }
        if let Some(self_ty) = cx.self_ty.clone()
            && let Some(def) = self.method_def(&self_ty, n)
        {
            let recv = V { ty: self_ty, taint: cx.self_taint };
            return self.user_method(cx, span, recv, def, argv, block);
        }
        if let Some(def) = self.fns.get(n).copied() {
            return self.fn_call(cx, span, def, None, argv, block);
        }
        if let Some(v) = self.builtin_function(cx, span, n, &argv, block) {
            return v;
        }
        if n == "run" {
            self.report(
                Diagnostic::new(name.span, "`run` can only be used inside an agent handler (`on Message … end`)")
                    .with_code(E_DECL),
            );
            return V::unknown();
        }
        self.walk_block(cx, block, &[]);
        let mut candidates = cx.names();
        candidates.extend(self.fns.keys().map(|s| s.to_string()));
        candidates.extend(builtins::GLOBALS.iter().map(|s| s.to_string()));
        self.error_help(
            E_NAME,
            name.span,
            format!("unknown function `{n}`"),
            suggest(n, candidates.iter().map(String::as_str)),
        );
        V::unknown()
    }

    /// Binds arguments to parameters; returns each parameter's taint.
    pub(crate) fn bind_args(
        &mut self,
        owner: &str,
        slots: &[Slot<'p>],
        args: &[ArgV],
        span: Span,
    ) -> Vec<Option<Span>> {
        let mut taints = vec![None; slots.len()];
        let mut used = vec![false; args.len()];
        let mut positional = args.iter().enumerate().filter(|(_, a)| a.name.is_none());
        for (i, slot) in slots.iter().enumerate() {
            let found = args
                .iter()
                .enumerate()
                .find(|(_, a)| a.name.as_deref() == Some(slot.name))
                .or_else(|| positional.next());
            match found {
                Some((j, arg)) => {
                    used[j] = true;
                    taints[i] = arg.v.taint;
                    if let Some(t) = slot.ty {
                        let expected = self.peek_ty(t);
                        if !self.compat(&arg.v.ty, &expected) {
                            self.error(
                                E_TYPE,
                                arg.span,
                                format!("`{owner}` expects `{expected}` for `{}`, got `{}`", slot.name, arg.v.ty),
                            );
                        }
                    }
                }
                None if slot.optional => {}
                None => self.error(E_TYPE, span, format!("missing argument `{}` for `{owner}`", slot.name)),
            }
        }
        for (j, arg) in args.iter().enumerate() {
            if used[j] {
                continue;
            }
            match &arg.name {
                Some(n) => self.error_help(
                    E_TYPE,
                    arg.span,
                    format!("unknown named argument `{n}:` for `{owner}`"),
                    suggest(n, slots.iter().map(|s| s.name)),
                ),
                None => {
                    self.error(E_TYPE, arg.span, format!("too many arguments for `{owner}` (expected {})", slots.len()))
                }
            }
        }
        taints
    }

    pub(crate) fn fn_call(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        def: &'p FnDef,
        recv: Option<V>,
        argv: Vec<ArgV>,
        block: Option<&'p Block>,
    ) -> V {
        if block.is_some() {
            self.error(E_TYPE, span, format!("`{}` does not take a block", def.name.name));
            self.walk_block(cx, block, &[]);
        }
        let mut taints = self.bind_args(&def.name.name, &Slot::params(&def.params), &argv, span);
        let mut recv = recv;
        if let Some(effect) = dangerous_effect(def) {
            for arg in &argv {
                if let Some(origin) = arg.v.taint {
                    self.taint_violation(arg.span, origin, &def.name.name, &effect);
                }
            }
            if let Some(origin) = recv.as_ref().and_then(|r| r.taint) {
                self.taint_violation(span, origin, &def.name.name, &effect);
            }
            // the call is rejected here (and at runtime): no need to report what follows inside the callee
            taints = declared_taints(&def.params);
            if let Some(r) = recv.as_mut() {
                r.taint = None;
            }
        }
        let self_ty = recv.as_ref().map(|r| r.ty.clone());
        let self_taint = recv.and_then(|r| r.taint);
        let (ret, effects) = self.check_fn(def, self_ty, taints, self_taint);
        for e in effects {
            cx.add_effect(Eff { origin: span, ..e });
        }
        if def.kind == FnKind::Prompt {
            return V { ty: ret.ty, taint: Some(span) };
        }
        ret
    }

    pub(crate) fn user_method(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        recv: V,
        def: &'p FnDef,
        argv: Vec<ArgV>,
        block: Option<&'p Block>,
    ) -> V {
        self.fn_call(cx, span, def, Some(recv), argv, block)
    }

    pub(crate) fn builtin_function(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        name: &str,
        argv: &[ArgV],
        block: Option<&'p Block>,
    ) -> Option<V> {
        let first = argv.first().map(|a| a.v.clone());
        let v = match name {
            "puts" | "print" | "warn" | "test" | "assert" | "assert_equal" => {
                self.walk_block(cx, block, &[]);
                V::new(Ty::Nil)
            }
            "p" => first.unwrap_or_else(|| V::new(Ty::Nil)),
            "raise" | "exit" => V::unknown(),
            "spawn" | "spawn_pool" => match first.map(|v| v.ty) {
                Some(Ty::Type(agent))
                    if self.types.get(agent.as_str()).is_some_and(|t| t.def.kind == TypeKind::Agent) =>
                {
                    V::new(Ty::User(agent))
                }
                Some(Ty::Unknown) => V::unknown(),
                Some(other) => {
                    self.error(E_TYPE, span, format!("`{name}` expects an agent type, got `{other}`"));
                    V::unknown()
                }
                None => {
                    self.error(E_TYPE, span, format!("`{name}` expects an agent: `{name} Researcher`"));
                    V::unknown()
                }
            },
            "budget" => {
                self.check_budget_options(argv);
                V::new(Ty::Budget)
            }
            "within" => {
                if let Some(v) = &first
                    && !self.compat(&v.ty, &Ty::Budget)
                {
                    self.error(E_TYPE, argv[0].span, format!("`within` expects a budget, got `{}`", v.ty));
                }
                self.walk_block(cx, block, &[]).unwrap_or_else(V::unknown)
            }
            "step" => {
                cx.steps += 1;
                let v = self.walk_block(cx, block, &[]).unwrap_or_else(V::unknown);
                cx.steps -= 1;
                v
            }
            "with_human" | "loop" => self.walk_block(cx, block, &[]).unwrap_or_else(V::unknown),
            "assert_raises" => {
                self.walk_block(cx, block, &[]);
                V::unknown()
            }
            "approve!" => {
                cx.add_effect(Eff { path: "human".into(), arg: None, origin: span });
                V::new(Ty::Nil)
            }
            "sleep" => {
                cx.add_effect(Eff { path: "time".into(), arg: None, origin: span });
                V::new(Ty::Nil)
            }
            "deny_all" | "approve_all" => V::new(Ty::Sym),
            _ => return None,
        };
        Some(v)
    }
}
