//! Agents: `ask` and the `run` loop.

use grenat_ast::{Arg, Diagnostic, Expr, ExprKind, FnDef, FnKind, Handler, Span};

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn ask(&mut self, cx: &mut Ctx<'p>, span: Span, agent: &str, argv: Vec<ArgV>) -> V {
        let Some(message) = argv.first() else {
            self.error(E_TYPE, span, "`ask` expects a message: `ask(Research(topic: t))`");
            return V::unknown();
        };
        let Ty::User(msg) = &message.v.ty else { return V::unknown() };
        let decl = &self.types[agent];
        let Some(handler) = decl.handlers.get(msg.as_str()).copied() else {
            let known: Vec<&str> = decl.handlers.keys().copied().collect();
            self.error_help(
                E_TYPE,
                message.span,
                format!("agent `{agent}` does not handle `{msg}`"),
                suggest(msg, known.clone()).or_else(|| Some(format!("handled messages: {}", known.join(", ")))),
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

    /// `run "instruction"`: agentic loop; tainted result, effects of its tools.
    pub(crate) fn run(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        agent: &'p str,
        handler: &'p Handler,
        argv: Vec<ArgV>,
    ) -> V {
        if argv.is_empty() {
            self.error(E_TYPE, span, "`run` expects an instruction: `run \"…\"`");
        }
        self.secrets_to_model(&argv, "run");
        cx.run_span = Some(span);
        match &handler.ret {
            Some(ret) if !is_tainted_decl(ret) => self.report(
                Diagnostic::new(
                    ret.span(),
                    "this handler returns the result of `run`, which comes from an LLM: its type must be tainted",
                )
                .with_code(E_TAINT_DECL)
                .with_note(span, "produced here")
                .with_help(format!("write `-> ~{}`", type_name(ret))),
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
        // an MCP server's tools: `mcp("<server>")`
        let servers: Vec<String> = self.types[agent]
            .directives
            .iter()
            .filter(|d| d.name.name == "tools")
            .flat_map(|d| d.args.iter())
            .filter_map(|a| match a {
                Arg::Pos(Expr { kind: ExprKind::Call { recv: None, name, args, .. }, .. }) if name.name == "mcp" => {
                    args.iter().find_map(|a| match a {
                        Arg::Pos(Expr { kind: ExprKind::Symbol(s), .. }) => Some(s.clone()),
                        _ => None,
                    })
                }
                _ => None,
            })
            .collect();
        for server in servers {
            cx.add_effect(Eff { path: "mcp".into(), arg: Some(server), origin: span });
        }
        for tool in tools {
            // arguments given by the LLM are validated against the schema: not tainted
            let (_, effects) = self.check_fn(tool, None, declared_taints(&tool.params), None);
            for e in effects {
                cx.add_effect(Eff { origin: span, ..e });
            }
        }
        let ty = handler.ret.as_ref().map_or(Ty::Str, |t| self.peek_ty(t));
        V { ty, taint: Some(span) }
    }
}
