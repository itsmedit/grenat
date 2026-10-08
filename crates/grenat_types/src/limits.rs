//! Rate limits, checked: `limit net("api.hubspot.com"), 10.per_second,
//! concurrent: 4`. `net(…)` names a host, as `uses net(…)` does: a trusted
//! `String` (E0412 otherwise), never a secret (E0414), a literal one a host
//! and nothing more (no scheme, port, path or user). The rate is a `Rate`
//! (`10.per_second`, `per_minute`, `per_hour`, `per_day`, `100.per(15.minutes)`),
//! a literal count at least one; `concurrent:` a positive `Int`; one of the
//! two at least. A host is limited once: two top-level declarations of the
//! same literal host are an error, as they are when the program runs.
//! Declaring calls nothing: it has no effect.

use grenat_ast::{Arg, Expr, ExprKind, Item, Span};

use crate::ty::{Ty, V};
use crate::*;

const USAGE: &str = "`limit net(\"api.github.com\"), 5000.per_hour, concurrent: 4`";

/// The methods of `Int` that make a rate.
pub(crate) const RATE_METHODS: [&str; 5] = ["per_second", "per_minute", "per_hour", "per_day", "per"];

/// `net("host")` as the first argument of `limit`: the host's expression
/// (mirrors `grenat_interp`'s `eval::limits::net_host`).
pub(crate) fn net_host(args: &[Arg]) -> Option<&Expr> {
    let Some(Arg::Pos(first)) = args.first() else {
        return None;
    };
    match &first.kind {
        ExprKind::Call { recv: None, name, args, block: None, .. } if name.name == "net" => match args.as_slice() {
            [Arg::Pos(host)] => Some(host),
            _ => None,
        },
        _ => None,
    }
}

/// A host as `uses net("…")` names it: no scheme, port, path, user or space.
fn valid_host(host: &str) -> bool {
    !host.is_empty() && !host.contains(['/', '@', '?', '#', ':']) && !host.chars().any(char::is_whitespace)
}

/// The count of a rate written as such (`0.per_second` → 0).
fn literal_count(e: &Expr) -> Option<f64> {
    match &e.kind {
        ExprKind::Call { recv: Some(recv), name, .. } if RATE_METHODS.contains(&name.name.as_str()) => {
            literal_number(recv)
        }
        _ => None,
    }
}

impl<'p> Checker<'p> {
    /// Two top-level `limit net("…")` of one literal host: the second is an
    /// error (the interpreter refuses it when the program loads).
    pub(crate) fn limits_declared_once(&mut self) {
        let mut hosts: Vec<String> = Vec::new();
        for item in &self.program.items {
            let Item::Stmt(Expr { kind: ExprKind::Call { recv: None, name, args, .. }, .. }) = item else {
                continue;
            };
            let Some(host) = net_host(args).filter(|_| name.name == "limit") else { continue };
            let Some(text) = literal_string(host).map(|h| h.to_ascii_lowercase()) else { continue };
            if hosts.contains(&text) {
                self.error(E_TYPE, host.span, format!("`net(\"{text}\")` is already limited: declare its limit once"));
            } else {
                hosts.push(text);
            }
        }
    }

    /// `limit net("api.hubspot.com"), 10.per_second, concurrent: 4`.
    pub(crate) fn limit_call(&mut self, cx: &mut Ctx<'p>, span: Span, host: &'p Expr, rest: &'p [Arg]) -> V {
        let v = self.expr(cx, host);
        if secrets::is_secret(&v.ty) {
            self.secret_as_string(host.span, "limit", "net(…)");
        } else if !self.compat(&v.ty, &Ty::Str) {
            self.error(E_TYPE, host.span, format!("`limit` expects a host (a `String`) in `net(…)`, got `{}`", v.ty));
        } else if let Some(text) = literal_string(host)
            && !valid_host(&text)
        {
            self.error(E_TYPE, host.span, format!("`limit` expects a host (`api.github.com`), got {text:?}"));
        }
        if let Some(origin) = v.taint {
            self.taint_violation(host.span, origin, "limit", "net");
        }
        let argv = self.args(cx, rest);
        let rates: Vec<&Expr> = rest
            .iter()
            .filter_map(|a| match a {
                Arg::Pos(e) => Some(e),
                _ => None,
            })
            .collect();
        let positional: Vec<&ArgV> = argv.iter().filter(|a| a.name.is_none()).collect();
        match (positional.as_slice(), rates.as_slice()) {
            ([], _) => {}
            ([rate], [expr]) => {
                if !self.compat(&rate.v.ty, &Ty::User(builtins::RATE.into())) {
                    let message = format!("`limit` expects a rate (`10.per_second`), got `{}`", rate.v.ty);
                    self.error(E_TYPE, rate.span, message);
                } else if literal_count(expr).is_some_and(|n| n < 1.0) {
                    self.error(E_TYPE, rate.span, "a rate allows at least one call");
                }
            }
            _ => self.error(E_TYPE, span, format!("`limit` takes a host, then a rate: {USAGE}")),
        }
        let mut concurrent = false;
        for arg in argv.iter().filter(|a| a.name.is_some()) {
            match arg.name.as_deref().unwrap_or_default() {
                "concurrent" => {
                    concurrent = true;
                    let literal_bad = arg.number.is_some_and(|n| n < 1.0 || n.fract() != 0.0);
                    if !self.compat(&arg.v.ty, &Ty::Int) || literal_bad {
                        let message = format!("`limit` expects `concurrent:` a positive count, got `{}`", arg.v.ty);
                        self.error(E_TYPE, arg.span, message);
                    }
                }
                name => {
                    let message = format!("`limit` has no option `{name}:` (it takes `concurrent:`)");
                    self.error(E_TYPE, arg.span, message);
                }
            }
        }
        for arg in &argv {
            if let Some(origin) = arg.v.taint {
                self.taint_violation(arg.span, origin, "limit", "net");
            }
        }
        if positional.is_empty() && !concurrent {
            self.error(E_TYPE, span, format!("`limit` expects a rate, `concurrent:`, or both: {USAGE}"));
        }
        V::new(Ty::Nil)
    }
}
