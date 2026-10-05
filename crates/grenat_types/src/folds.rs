//! Array methods whose result a literal argument decides, typed from it:
//! `flatten(depth)` takes `depth` levels off (none for `0`, all for a
//! negative one, unknown below for a depth computed at run time), and
//! `reduce(:op)` / `inject(init, :op)` / `reduce(&:op)` make what the
//! operator makes — a comparison a `Bool` (`:<`, `:==`), `:<=>` an `Int`,
//! arithmetic the items' kind. Their arguments are checked as the
//! interpreter takes them (E0200).

use grenat_ast::{Arg, Expr, ExprKind, Ident, UnOp};

use crate::ty::{Ty, V, join};
use crate::*;

impl<'p> Checker<'p> {
    /// `v`, the type `builtins::method` gave `recv.name(args…)`, refined by
    /// its literal arguments when `recv` is an array or a range.
    pub(crate) fn fold_typed(
        &mut self,
        recv: &Ty,
        name: &Ident,
        args: &'p [Arg],
        arg_types: &[Ty],
        block: bool,
        v: V,
    ) -> V {
        let elem = match recv.base() {
            Ty::Array(t) => (**t).clone(),
            Ty::Range => Ty::Int,
            _ => return v,
        };
        let positional: Vec<&Expr> = args
            .iter()
            .filter_map(|a| match a {
                Arg::Pos(e) => Some(e),
                _ => None,
            })
            .collect();
        let passed = args.iter().find_map(|a| match a {
            Arg::BlockPass(e) => Some(e),
            _ => None,
        });
        let ty = match name.name.as_str() {
            "flatten" => self.flatten_typed(&elem, name, &positional, arg_types),
            "reduce" | "inject" if block => {
                self.at_most_one_init(name, positional.len());
                return v;
            }
            "reduce" | "inject" => match passed {
                Some(op) => {
                    self.at_most_one_init(name, positional.len());
                    let acc = arg_types.first().unwrap_or(&elem);
                    symbol(op).map_or(v.ty, |op| operated(op, acc, &elem))
                }
                None => self.reduce_typed(&elem, name, &positional, arg_types),
            },
            _ => return v,
        };
        V { ty, taint: v.taint }
    }

    /// `flatten`, `flatten(depth)`.
    fn flatten_typed(&mut self, elem: &Ty, name: &Ident, positional: &[&Expr], arg_types: &[Ty]) -> Ty {
        match (positional, arg_types) {
            ([], _) => flattened(elem, None),
            ([depth], [ty]) => {
                if !self.compat(ty, &Ty::Int) {
                    self.error(E_TYPE, name.span, format!("`flatten` expects an `Int` depth, got `{ty}`"));
                }
                match integer(depth) {
                    Some(d) => flattened(elem, Some(d)),
                    None => Ty::array(Ty::Unknown),
                }
            }
            _ => {
                let n = positional.len();
                self.error(E_TYPE, name.span, format!("`flatten` takes one depth at most, got {n}"));
                Ty::array(Ty::Unknown)
            }
        }
    }

    /// `reduce(:op)`, `inject(init, :op)`: without a block, the last
    /// argument names the operator.
    fn reduce_typed(&mut self, elem: &Ty, name: &Ident, positional: &[&Expr], arg_types: &[Ty]) -> Ty {
        let n = &name.name;
        let operator = arg_types.last().filter(|t| matches!(t.base(), Ty::Sym | Ty::Unknown));
        if positional.is_empty() || positional.len() > 2 || operator.is_none() {
            self.error(
                E_TYPE,
                name.span,
                format!("`{n}` expects a block or an operator: `{n}(:+)`, `{n}(0, :+)`, `{n} {{ |acc, x| … }}`"),
            );
            return Ty::Unknown;
        }
        let acc = if positional.len() == 2 { &arg_types[0] } else { elem };
        positional.last().and_then(|op| symbol(op)).map_or(Ty::Unknown, |op| operated(op, acc, elem))
    }

    /// With a block, `reduce` takes one initial value at most.
    fn at_most_one_init(&mut self, name: &Ident, given: usize) {
        if given > 1 {
            self.error(
                E_TYPE,
                name.span,
                format!("`{}` with a block takes one initial value at most, got {given}", name.name),
            );
        }
    }
}

/// The type of `Array(elem).flatten(depth)`: `depth` levels off, all of
/// them for `None` or a negative depth.
pub(crate) fn flattened(elem: &Ty, depth: Option<i64>) -> Ty {
    match (elem.base(), depth) {
        (_, Some(0)) => Ty::array(elem.clone()),
        (Ty::Array(inner), Some(d)) if d > 0 => flattened(inner, Some(d - 1)),
        (Ty::Array(inner), _) => flattened(inner, None),
        (other, _) => Ty::array(other.clone()),
    }
}

/// What `acc op item` makes, the operator named by a symbol.
fn operated(op: &str, acc: &Ty, elem: &Ty) -> Ty {
    match op {
        "+" | "-" | "*" | "/" | "%" | "**" => join(acc, elem),
        "<" | "<=" | ">" | ">=" | "==" | "!=" => Ty::Bool,
        "<=>" => Ty::Int,
        // a method named by the symbol: unknown
        _ => Ty::Unknown,
    }
}

/// A literal symbol's name: `:+`.
fn symbol(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Symbol(name) => Some(name),
        _ => None,
    }
}

/// A literal integer, maybe negative: `2`, `-1`.
fn integer(e: &Expr) -> Option<i64> {
    match &e.kind {
        ExprKind::Int(n) => Some(*n),
        ExprKind::Unary { op: UnOp::Neg, expr } => integer(expr).and_then(i64::checked_neg),
        _ => None,
    }
}
