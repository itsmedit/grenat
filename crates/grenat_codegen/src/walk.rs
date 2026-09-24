//! Traversal of the expressions of a compiled body.

use grenat_ast::{Arg, Expr, ExprKind, StrSeg};

/// Calls `f` on `e` and on every expression nested in it (blocks included).
pub(crate) fn each(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Str(segs) => {
            for seg in segs {
                if let StrSeg::Interp(e) = seg {
                    each(e, f);
                }
            }
        }
        ExprKind::Array(items) => each_in(items, f),
        ExprKind::Assign { target, value } | ExprKind::OpAssign { target, value, .. } => {
            each(target, f);
            each(value, f);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            each(lhs, f);
            each(rhs, f);
        }
        ExprKind::Unary { expr, .. } => each(expr, f),
        ExprKind::If { cond, then, else_ } => {
            each(cond, f);
            each_in(then, f);
            if let Some(else_) = else_ {
                each_in(else_, f);
            }
        }
        ExprKind::While { cond, body } => {
            each(cond, f);
            each_in(body, f);
        }
        ExprKind::Return(Some(value)) => each(value, f),
        ExprKind::Index { recv, args } => {
            each(recv, f);
            each_in(args, f);
        }
        ExprKind::Call { recv, args, block, .. } => {
            if let Some(recv) = recv {
                each(recv, f);
            }
            for arg in args {
                if let Arg::Pos(e) | Arg::BlockPass(e) | Arg::Named { value: Some(e), .. } = arg {
                    each(e, f);
                }
            }
            if let Some(block) = block {
                each_in(&block.body.stmts, f);
            }
        }
        _ => {}
    }
}

fn each_in(es: &[Expr], f: &mut impl FnMut(&Expr)) {
    for e in es {
        each(e, f);
    }
}

/// `e` mentions variable `name` (reads or assigns it).
pub(crate) fn mentions(e: &Expr, name: &str) -> bool {
    let mut found = false;
    each(e, &mut |x| match &x.kind {
        ExprKind::Var(n) => found |= n == name,
        ExprKind::Call { args, .. } => {
            // `field:` alone reads the variable of that name
            found |= args.iter().any(|a| matches!(a, Arg::Named { name: n, value: None } if n.name == name));
        }
        _ => {}
    });
    found
}

/// The literal pieces of every string in `stmts`.
pub(crate) fn string_literals(stmts: &[Expr]) -> Vec<String> {
    let mut out = Vec::new();
    for stmt in stmts {
        each(stmt, &mut |x| {
            if let ExprKind::Str(segs) = &x.kind {
                out.extend(segs.iter().filter_map(|s| match s {
                    StrSeg::Lit(text) => Some(text.clone()),
                    StrSeg::Interp(_) => None,
                }));
            }
        });
    }
    out
}
