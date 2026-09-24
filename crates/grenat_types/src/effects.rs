//! Effects: representation, coverage by a `uses` declaration, and a function's exported effects.

use std::collections::HashSet;

use grenat_ast::{Diagnostic, Expr, ExprKind, FnDef, FnKind, Span, StrSeg};

use crate::*;

pub(crate) const KNOWN_EFFECTS: &[&str] =
    &["llm", "net", "fs", "fs.read", "fs.write", "shell", "human", "time", "random", "env"];

/// Inferred or declared effect: `fs.read("./docs")`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Eff {
    pub(crate) path: String,
    pub(crate) arg: Option<String>,
    pub(crate) origin: Span,
}

impl Eff {
    pub(crate) fn label(&self) -> String {
        match &self.arg {
            Some(arg) => format!("{}(\"{arg}\")", self.path),
            None => self.path.clone(),
        }
    }
}

pub(crate) fn normalize_path(p: &str) -> String {
    let trimmed = p.trim_start_matches("./").trim_end_matches('/');
    if trimmed.is_empty() { ".".into() } else { trimmed.to_string() }
}

/// Does `declared` allow `used`? A dynamic restriction is checked at runtime.
pub(crate) fn covers(declared: &Eff, used: &Eff) -> bool {
    let path_ok = declared.path == used.path || used.path.starts_with(&format!("{}.", declared.path));
    path_ok
        && match (&declared.arg, &used.arg) {
            (None, _) | (Some(_), None) => true,
            (Some(d), Some(u)) if used.path == "net" => d == u,
            (Some(d), Some(u)) => {
                let (d, u) = (normalize_path(d), normalize_path(u));
                d == "." || u == d || u.starts_with(&format!("{d}/"))
            }
        }
}

pub(crate) fn literal_string(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Str(segs) => segs
            .iter()
            .map(|s| match s {
                StrSeg::Lit(t) => Some(t.as_str()),
                StrSeg::Interp(_) => None,
            })
            .collect(),
        _ => None,
    }
}

impl<'p> Checker<'p> {
    pub(crate) fn effect_count(&self) -> usize {
        self.prev_effects.values().map(Vec::len).sum()
    }

    /// Exported effects: the declared ones (checked), otherwise the inferred ones.
    pub(crate) fn finish_effects(&mut self, def: &'p FnDef, cx: &Ctx<'p>) -> Vec<Eff> {
        let mut inferred = cx.effects.clone();
        if def.kind == FnKind::Prompt {
            inferred.push(Eff { path: "llm".into(), arg: None, origin: def.span });
        }
        let is_main = def.name.name == "main" && self.fns.get("main").is_some_and(|m| std::ptr::eq(*m, def));
        if def.effects.is_empty() {
            if (def.kind == FnKind::Tool || is_main) && !inferred.is_empty() {
                let what = if is_main { "`main` is the capability root" } else { "a tool" };
                let list: Vec<String> =
                    inferred.iter().map(|e| e.path.clone()).collect::<HashSet<_>>().into_iter().collect();
                let mut list = list;
                list.sort();
                self.report(
                    Diagnostic::new(
                        inferred[0].origin,
                        format!("`{}` uses effect `{}` without declaring it", def.name.name, inferred[0].label()),
                    )
                    .with_code(E_EFFECT)
                    .with_note(def.name.span, format!("{what}: its effects must be declared"))
                    .with_help(format!("add `uses {}`", list.join(", "))),
                );
            }
            return inferred;
        }
        let declared: Vec<Eff> = def
            .effects
            .iter()
            .map(|e| Eff {
                path: e.path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join("."),
                arg: e.args.first().and_then(literal_string),
                origin: e.span,
            })
            .collect();
        for used in &inferred {
            if !declared.iter().any(|d| covers(d, used)) {
                let list: Vec<String> = declared.iter().map(Eff::label).collect();
                self.report(
                    Diagnostic::new(
                        used.origin,
                        format!("`{}` uses effect `{}` without declaring it", def.name.name, used.label()),
                    )
                    .with_code(E_EFFECT)
                    .with_note(def.name.span, format!("`{}` declares: uses {}", def.name.name, list.join(", ")))
                    .with_help(format!("add `{}` to `uses`", used.label())),
                );
            }
        }
        declared
    }
}
