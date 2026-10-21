//! Rate limits, checked: `limit net("api.hubspot.com"), 10.per_second,
//! concurrent: 4`. `net(…)` names a host, as `uses net(…)` does: a trusted
//! `String` (E0412 otherwise), never a secret (E0414), a literal one a host
//! and nothing more (no scheme, port, path or user). The rate is a `Rate`
//! (`10.per_second`, `per_minute`, `per_hour`, `per_day`, `100.per(15.minutes)`),
//! a literal one as the runtime holds it (one call at least, `u32::MAX` at
//! most, a positive period of a year at most — wherever it is written);
//! `concurrent:` a positive `Int`; one of the two at least.
//!
//! **Where.** `limit` is a statement at the top level of the program, or at
//! the top of a `test` block (a limit of its own, gone when it ends):
//! anywhere else — a function, a block, a loop — it could run twice, and a
//! host is limited once (E0500). A host is limited once: two declarations
//! of the same literal host at the top level, or one in a test of a host
//! limited at the top level or earlier in that test, are an error, as they
//! are when the program runs. Declaring calls nothing: it has no effect.

use grenat_ast::{Arg, Diagnostic, Expr, ExprKind, Item, Span};
use grenat_limits::Rate;

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

/// A rate written as such (`10.per_second`, `100.per(15.minutes)`): its
/// count and its period in seconds.
fn literal_rate(recv: &Expr, name: &str, args: &[Arg]) -> Option<(i64, f64)> {
    let ExprKind::Int(count) = recv.kind else { return None };
    let seconds = match (name, args) {
        ("per_second", []) => 1.0,
        ("per_minute", []) => 60.0,
        ("per_hour", []) => 3600.0,
        ("per_day", []) => 86_400.0,
        ("per", [Arg::Pos(period)]) => literal_number(period)?,
        _ => return None,
    };
    Some((count, seconds))
}

/// The host of `stmt` if it is `limit net("…")` written out (lowercase).
fn limited_host(stmt: &Expr) -> Option<(&Expr, Option<String>)> {
    let ExprKind::Call { recv: None, name, args, .. } = &stmt.kind else { return None };
    let host = net_host(args).filter(|_| name.name == "limit")?;
    Some((host, literal_string(host).map(|h| h.to_ascii_lowercase())))
}

/// The statements of `test "…" do … end`.
fn test_body(stmt: &Expr) -> Option<&[Expr]> {
    match &stmt.kind {
        ExprKind::Call { recv: None, name, block: Some(block), .. } if name.name == "test" => Some(&block.body.stmts),
        _ => None,
    }
}

impl<'p> Checker<'p> {
    /// Where `limit` may be (the top level, the top of a test), and each
    /// literal host limited once there: a second declaration is an error,
    /// as the interpreter refuses it when it runs.
    pub(crate) fn limits_declared_once(&mut self) {
        let program = self.program;
        let stmts: Vec<&Expr> = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Stmt(stmt) => Some(stmt),
                _ => None,
            })
            .collect();
        let mut top: Vec<String> = Vec::new();
        for stmt in &stmts {
            self.limit_in_place(stmt, &mut top);
        }
        for body in stmts.iter().filter_map(|stmt| test_body(stmt)) {
            let mut hosts = top.clone();
            for stmt in body {
                self.limit_in_place(stmt, &mut hosts);
            }
        }
    }

    /// `stmt`, if a `limit`, is where one may be; its host is limited once
    /// among `hosts`.
    fn limit_in_place(&mut self, stmt: &Expr, hosts: &mut Vec<String>) {
        let Some((host, text)) = limited_host(stmt) else { return };
        self.limit_places.insert((stmt.span.start, stmt.span.end));
        let Some(text) = text else { return };
        if hosts.contains(&text) {
            self.error(E_TYPE, host.span, format!("`net(\"{text}\")` is already limited: declare its limit once"));
        } else {
            hosts.push(text);
        }
    }

    /// A rate written out, held to what the runtime allows (`10.per(0.s)`,
    /// `1.per(400.days)`, `5_000_000_000.per_second` are not rates).
    pub(crate) fn rate_literal(&mut self, span: Span, recv: &Expr, name: &str, args: &[Arg]) {
        if let Some((count, seconds)) = literal_rate(recv, name, args)
            && let Err(why) = Rate::new(count, seconds)
        {
            self.error(E_TYPE, span, why);
        }
    }

    /// `limit net("api.hubspot.com"), 10.per_second, concurrent: 4`.
    pub(crate) fn limit_call(&mut self, cx: &mut Ctx<'p>, span: Span, host: &'p Expr, rest: &'p [Arg]) -> V {
        if !self.limit_places.contains(&(span.start, span.end)) {
            self.report(
                Diagnostic::new(span, "`limit` is declared at the top level of the program, or of a test")
                    .with_code(E_DECL)
                    .with_help(
                        "in a function, a block or a loop it could run twice, and a host is limited once: move it out",
                    ),
            );
        }
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
            ([rate], [_]) => {
                if !self.compat(&rate.v.ty, &Ty::User(builtins::RATE.into())) {
                    let message = format!("`limit` expects a rate (`10.per_second`), got `{}`", rate.v.ty);
                    self.error(E_TYPE, rate.span, message);
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
