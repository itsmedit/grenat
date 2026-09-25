//! `require "…"` statements: found at the top level of a file, resolved
//! before the program runs, then removed from it.

use grenat_ast::{Arg, Expr, ExprKind, Item, Program, Span, StrSeg};

/// A `require` of a file: its target as written, and where.
pub(crate) struct Require {
    pub target: Result<String, &'static str>,
    pub span: Span,
}

/// The `require` statements of a program.
pub(crate) fn requires(program: &Program) -> Vec<Require> {
    program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Stmt(expr) if is_require_expr(expr) => Some(Require { target: target(expr), span: expr.span }),
            _ => None,
        })
        .collect()
}

/// Whether `item` is a `require` (removed from the program once resolved).
pub fn is_require(item: &Item) -> bool {
    matches!(item, Item::Stmt(expr) if is_require_expr(expr))
}

/// The program without its `require` statements.
pub fn strip_requires(program: &mut Program) {
    program.items.retain(|item| !is_require(item));
}

fn is_require_expr(expr: &Expr) -> bool {
    matches!(&expr.kind, ExprKind::Call { recv: None, name, .. } if name.name == "require")
}

fn target(expr: &Expr) -> Result<String, &'static str> {
    const USAGE: &str = "`require` takes a path: `require \"./helpers\"` or `require \"package\"`";
    let ExprKind::Call { args, block: None, .. } = &expr.kind else { return Err(USAGE) };
    match args.as_slice() {
        [Arg::Pos(Expr { kind: ExprKind::Str(segments), .. })] => match segments.as_slice() {
            [StrSeg::Lit(path)] if !path.is_empty() => Ok(path.clone()),
            _ => Err("`require` takes a literal path, without interpolation"),
        },
        _ => Err(USAGE),
    }
}
