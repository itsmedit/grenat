//! Agents : `ask` et la boucle `run`.

use grenat_ast::{Arg, Diagnostic, Expr, ExprKind, FnDef, FnKind, Handler, Span};

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn ask(&mut self, cx: &mut Ctx<'p>, span: Span, agent: &str, argv: Vec<ArgV>) -> V {
        let Some(message) = argv.first() else {
            self.error(E_TYPE, span, "`ask` attend un message : `ask(Research(topic: t))`");
            return V::unknown();
        };
        let Ty::User(msg) = &message.v.ty else { return V::unknown() };
        let decl = &self.types[agent];
        let Some(handler) = decl.handlers.get(msg.as_str()).copied() else {
            let known: Vec<&str> = decl.handlers.keys().copied().collect();
            self.error_help(
                E_TYPE,
                message.span,
                format!("l'agent `{agent}` ne gère pas `{msg}`"),
                suggest(msg, known.clone()).or_else(|| Some(format!("messages gérés : {}", known.join(", ")))),
            );
            return V::unknown();
        };
        let agent: &'p str = decl.def.name.name.as_str();
        let taints = vec![message.v.taint; handler.params.len()];
        let (ret, effects) = self.check_handler(agent, handler, taints);
        for e in effects {
            cx.add_effect(Eff { origin: span, ..e });
        }
        ret
    }

    /// `run "consigne"` : boucle agentique ; résultat teinté, effets de ses outils.
    pub(crate) fn run(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        agent: &'p str,
        handler: &'p Handler,
        argv: Vec<ArgV>,
    ) -> V {
        if argv.is_empty() {
            self.error(E_TYPE, span, "`run` attend une consigne : `run \"…\"`");
        }
        cx.run_span = Some(span);
        match &handler.ret {
            Some(ret) if !is_tainted_decl(ret) => self.report(
                Diagnostic::new(
                    ret.span(),
                    "ce handler renvoie le résultat de `run`, qui vient d'un LLM : son type doit être teinté",
                )
                .with_code(E_TAINT_DECL)
                .with_note(span, "résultat produit ici")
                .with_help(format!("écrivez `-> ~{}`", type_name(ret))),
            ),
            Some(ret) => {
                let ty = self.peek_ty(ret);
                if let Err(e) = self.schema_ok(&ty, 0) {
                    self.error(E_DECL, ret.span(), e);
                }
            }
            None => {}
        }
        cx.add_effect(Eff { path: "llm".into(), arg: None, origin: span });
        let tools: Vec<&'p FnDef> = self.types[agent]
            .directives
            .iter()
            .filter(|d| d.name.name == "tools")
            .flat_map(|d| d.args.iter())
            .filter_map(|a| match a {
                Arg::Pos(Expr { kind: ExprKind::Var(name), .. }) => self.fns.get(name.as_str()).copied(),
                _ => None,
            })
            .filter(|f| f.kind == FnKind::Tool)
            .collect();
        for tool in tools {
            // les arguments donnés par le LLM sont validés par le schéma : non teintés
            let (_, effects) = self.check_fn(tool, None, declared_taints(&tool.params), None);
            for e in effects {
                cx.add_effect(Eff { origin: span, ..e });
            }
        }
        let ty = handler.ret.as_ref().map_or(Ty::Str, |t| self.peek_ty(t));
        V { ty, taint: Some(span) }
    }
}
