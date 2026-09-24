//! Vérification des déclarations : modèles, fonctions, types, agents, superviseurs.

use grenat_ast::{
    Arg, Diagnostic, Directive, Expr, ExprKind, FnDef, FnKind, Handler, Item, Member, Span, TypeDef, TypeKind,
};

use crate::ty::Ty;
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn pass(&mut self) {
        let program = self.program;
        for item in &program.items {
            match item {
                Item::Fn(def) => {
                    self.check_fn_decl(def);
                    let taints = declared_taints(&def.params);
                    self.check_fn(def, None, taints, None);
                }
                Item::Type(def) => self.check_type(def),
                Item::Model(model) => self.check_model(model),
                Item::Stmt(_) => {}
            }
        }
        let mut cx = Ctx::new(Kind::Top, None, None);
        for item in &program.items {
            if let Item::Stmt(e) = item {
                self.expr(&mut cx, e);
            }
        }
    }

    pub(crate) fn check_model(&mut self, model: &'p grenat_ast::ModelDecl) {
        for option in &model.options {
            let Arg::Named { name, value: Some(value) } = option else { continue };
            match (name.name.as_str(), &value.kind) {
                ("provider", ExprKind::Symbol(p)) if p != "anthropic" => self.error(
                    E_DECL,
                    value.span,
                    format!("fournisseur `:{p}` non pris en charge (disponible : `:anthropic`)"),
                ),
                ("provider" | "name" | "temperature" | "max_tokens" | "effort" | "fallbacks", _) => {}
                (other, _) => self.error_help(
                    E_DECL,
                    name.span,
                    format!("option de modèle inconnue `{other}:`"),
                    suggest(other, ["provider", "name", "temperature", "max_tokens", "effort", "fallbacks"]),
                ),
            }
        }
    }

    pub(crate) fn check_fn_decl(&mut self, def: &'p FnDef) {
        for effect in &def.effects {
            let path: Vec<&str> = effect.path.iter().map(|i| i.name.as_str()).collect();
            let path = path.join(".");
            if !KNOWN_EFFECTS.contains(&path.as_str()) {
                self.error_help(
                    E_DECL,
                    effect.span,
                    format!("effet inconnu `{path}`"),
                    suggest(&path, KNOWN_EFFECTS.iter().copied()),
                );
            }
        }
        if def.kind == FnKind::Tool {
            for param in &def.params {
                match &param.ty {
                    None => self.error(
                        E_DECL,
                        param.span,
                        format!("le paramètre `{}` d'un outil doit être typé", param.name.name),
                    ),
                    Some(t) => {
                        let ty = self.resolve(t).0;
                        if let Err(e) = self.schema_ok(&ty, 0) {
                            self.error(E_DECL, t.span(), e);
                        }
                    }
                }
            }
        }
        if def.kind == FnKind::Prompt {
            if let Some(ret) = &def.ret {
                if !is_tainted_decl(ret) {
                    self.report(
                        Diagnostic::new(
                            ret.span(),
                            "le résultat d'un `prompt` vient d'un LLM : son type doit être teinté",
                        )
                        .with_code(E_TAINT_DECL)
                        .with_help(format!("écrivez `-> ~{}`", type_name(ret))),
                    );
                }
                let ty = self.resolve(ret).0;
                if let Err(e) = self.schema_ok(&ty, 0) {
                    self.error(E_DECL, ret.span(), e);
                }
            }
            self.check_model_ref(def.model.as_ref(), def.name.span);
        }
    }

    pub(crate) fn check_model_ref(&mut self, selector: Option<&'p Expr>, span: Span) {
        match selector.map(|e| (&e.kind, e.span)) {
            Some((ExprKind::Symbol(name), span)) if !self.models.contains(&name.as_str()) => {
                let models = self.models.clone();
                self.error_help(E_NAME, span, format!("modèle `:{name}` non déclaré"), suggest(name, models));
            }
            None if self.models.is_empty() => self.report(
                Diagnostic::new(span, "aucun modèle déclaré")
                    .with_code(E_DECL)
                    .with_help("ajoutez `model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"`"),
            ),
            _ => {}
        }
    }

    pub(crate) fn check_type(&mut self, def: &'p TypeDef) {
        let name: &'p str = &def.name.name;
        let decl = &self.types[name];
        let (fields, ivars, variants) = (decl.fields.clone(), decl.ivars.clone(), decl.variants.clone());
        let methods: Vec<&'p FnDef> = def
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Method(f) => Some(f),
                _ => None,
            })
            .collect();
        let handlers: Vec<&'p Handler> = decl.handlers.values().copied().collect();
        let directives = decl.directives.clone();

        for f in fields.iter().chain(variants.iter().flat_map(|v| v.fields.iter()).collect::<Vec<_>>().iter()) {
            if let Some(t) = &f.ty {
                self.resolve(t);
            }
        }
        let self_ty = Ty::user(name);
        let mut cx = Ctx::new(Kind::Top, Some(self_ty.clone()), None);
        for f in fields.iter().chain(ivars.iter()) {
            let declared = f.ty.as_ref().map(|t| self.resolve(t).0);
            if let Some(default) = &f.default {
                let v = self.expr(&mut cx, default);
                if let Some(expected) = &declared
                    && !self.compat(&v.ty, expected)
                {
                    self.error(E_TYPE, default.span, format!("`{}` attend `{expected}`, reçu `{}`", f.name.name, v.ty));
                }
            }
        }
        for def in methods {
            self.check_fn_decl(def);
            let self_ty = if def.on_self { Ty::Type(name.into()) } else { self_ty.clone() };
            self.check_fn(def, Some(self_ty), declared_taints(&def.params), None);
        }
        match def.kind {
            TypeKind::Agent => {
                self.check_agent_directives(name, &directives);
                for handler in handlers {
                    self.check_handler(name, handler, declared_taints(&handler.params));
                }
            }
            TypeKind::Supervisor => {
                self.check_supervisor_options(&mut cx, &def.options);
                for d in directives {
                    match (d.name.name.as_str(), d.args.first()) {
                        ("child", Some(Arg::Pos(Expr { kind: ExprKind::Const(path), span }))) => {
                            let child = path.last().expect("chemin").name.as_str();
                            if !self.types.get(child).is_some_and(|t| t.def.kind == TypeKind::Agent) {
                                self.error(E_NAME, *span, format!("agent inconnu `{child}`"));
                            }
                        }
                        ("child", _) => self.error(E_DECL, d.span, "`child` attend un agent : `child Writer`"),
                        (other, _) => {
                            self.error(E_DECL, d.name.span, format!("directive de superviseur inconnue `{other}`"))
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn check_agent_directives(&mut self, agent: &'p str, directives: &[&'p Directive]) {
        let mut cx = Ctx::new(Kind::Top, Some(Ty::user(agent)), None);
        for d in directives {
            let first = d.args.iter().find_map(|a| match a {
                Arg::Pos(e) => Some(e),
                _ => None,
            });
            match d.name.name.as_str() {
                "model" => self.check_model_ref(first, d.span),
                "tools" => {
                    for arg in &d.args {
                        let Arg::Pos(Expr { kind: ExprKind::Var(tool), span }) = arg else {
                            self.error(E_DECL, d.span, "`tools` attend des noms d'outils : `tools lire, chercher`");
                            continue;
                        };
                        match self.fns.get(tool.as_str()) {
                            Some(def) if def.kind == FnKind::Tool => {}
                            Some(_) => self.report(
                                Diagnostic::new(*span, format!("`{tool}` n'est pas un `tool`"))
                                    .with_code(E_DECL)
                                    .with_help("déclarez-le avec `tool` : seuls les outils sont confiés à un LLM"),
                            ),
                            None => {
                                let tools: Vec<&str> =
                                    self.fns.iter().filter(|(_, f)| f.kind == FnKind::Tool).map(|(n, _)| *n).collect();
                                self.error_help(E_NAME, *span, format!("outil inconnu `{tool}`"), suggest(tool, tools));
                            }
                        }
                    }
                }
                "max_turns" => {
                    if let Some(e) = first {
                        let v = self.expr(&mut cx, e);
                        if !self.compat(&v.ty, &Ty::Int) {
                            self.error(E_TYPE, e.span, format!("`max_turns` attend un `Int`, reçu `{}`", v.ty));
                        }
                    }
                }
                "instructions" => {
                    if let Some(e) = first {
                        self.expr(&mut cx, e);
                    }
                }
                "budget" => {
                    let args = self.args(&mut cx, &d.args);
                    self.check_budget_options(&args);
                }
                other => self.error_help(
                    E_DECL,
                    d.name.span,
                    format!("directive d'agent inconnue `{other}`"),
                    suggest(other, ["model", "tools", "max_turns", "instructions", "budget"]),
                ),
            }
        }
    }

    pub(crate) fn check_budget_options(&mut self, args: &[ArgV]) {
        for a in args {
            let expected = match a.name.as_deref() {
                Some("usd") => Ty::Money,
                Some("tokens") => Ty::Int,
                Some("time") => Ty::Duration,
                Some(other) => {
                    self.error_help(
                        E_TYPE,
                        a.span,
                        format!("option de budget inconnue `{other}:`"),
                        suggest(other, ["usd", "tokens", "time"]),
                    );
                    continue;
                }
                None => {
                    self.error(
                        E_TYPE,
                        a.span,
                        "`budget` n'accepte que des options nommées : `usd:`, `tokens:`, `time:`",
                    );
                    continue;
                }
            };
            let ok = self.compat(&a.v.ty, &expected) || (expected == Ty::Duration && self.compat(&a.v.ty, &Ty::Int));
            if !ok {
                self.error(
                    E_TYPE,
                    a.span,
                    format!("`{}:` attend `{expected}`, reçu `{}`", a.name.as_deref().unwrap_or(""), a.v.ty),
                );
            }
        }
    }

    /// `supervisor Desk, strategy: :one_for_one, max_restarts: 3, within: 1.min`
    pub(crate) fn check_supervisor_options(&mut self, cx: &mut Ctx<'p>, options: &'p [Arg]) {
        const STRATEGIES: [&str; 3] = ["one_for_one", "one_for_all", "rest_for_one"];
        for option in options {
            let (name, value) = match option {
                Arg::Named { name, value: Some(value) } => (name, value),
                Arg::Pos(value) | Arg::BlockPass(value) => {
                    self.error(
                        E_DECL,
                        value.span,
                        "les options d'un superviseur sont nommées : `strategy:`, `max_restarts:`, `within:`",
                    );
                    continue;
                }
                Arg::Named { name, value: None } => {
                    self.error(E_DECL, name.span, format!("valeur attendue pour `{}:`", name.name));
                    continue;
                }
            };
            let v = self.expr(cx, value);
            match name.name.as_str() {
                "strategy" => match &value.kind {
                    ExprKind::Symbol(s) if STRATEGIES.contains(&s.as_str()) => {}
                    ExprKind::Symbol(s) => self.error_help(
                        E_DECL,
                        value.span,
                        format!("stratégie de supervision inconnue `:{s}`"),
                        suggest(s, STRATEGIES)
                            .or_else(|| Some("stratégies : :one_for_one, :one_for_all, :rest_for_one".into())),
                    ),
                    _ => self.error(E_TYPE, value.span, "`strategy:` attend un symbole, par exemple `:one_for_one`"),
                },
                "max_restarts" if !self.compat(&v.ty, &Ty::Int) => {
                    self.error(E_TYPE, value.span, format!("`max_restarts:` attend un `Int`, reçu `{}`", v.ty));
                }
                "within" if !(self.compat(&v.ty, &Ty::Duration) || self.compat(&v.ty, &Ty::Float)) => {
                    self.error(E_TYPE, value.span, format!("`within:` attend une durée (`1.min`), reçu `{}`", v.ty));
                }
                "max_restarts" | "within" => {}
                other => self.error_help(
                    E_DECL,
                    name.span,
                    format!("option de superviseur inconnue `{other}:`"),
                    suggest(other, ["strategy", "max_restarts", "within"]),
                ),
            }
        }
    }
}
