//! Literals, assignments and operators.

use grenat_ast::{BinOp, Expr, ExprKind, StrSeg, UnOp};

use super::{Flow, Infer, Reject};
use crate::ty::{Elem, Ty};

impl Infer<'_, '_> {
    pub(super) fn string(&mut self, segs: &[StrSeg]) -> Result<Ty, Reject> {
        for seg in segs {
            if let StrSeg::Interp(e) = seg {
                match self.value(e, None)? {
                    Ty::Int | Ty::Float | Ty::Bool | Ty::Str => {}
                    t => return Err(format!("interpolates a `{}`", self.show(t))),
                }
            }
        }
        Ok(Ty::Str)
    }

    pub(super) fn array(&mut self, items: &[Expr], expected: Option<Ty>) -> Result<Ty, Reject> {
        let expected = match expected {
            Some(Ty::Array(elem)) => Some(elem),
            _ => None,
        };
        let Some((first, rest)) = items.split_first() else {
            return Ok(Ty::Array(expected.unwrap_or(Elem::Unknown)));
        };
        let t = self.value(first, expected.and_then(Elem::ty))?;
        let Some(elem) = t.elem() else { return Err("puts an array inside an array".into()) };
        for item in rest {
            let other = self.value(item, Some(t))?;
            if other != t {
                return Err(format!("mixes `{}` and `{}` in an array", self.show(t), self.show(other)));
            }
        }
        Ok(Ty::Array(elem))
    }

    pub(super) fn assign(&mut self, target: &Expr, value: &Expr) -> Result<Flow, Reject> {
        match &target.kind {
            ExprKind::Var(name) => {
                let t = self.value(value, self.declared(name))?;
                self.define(name, t)?;
                // the value of an assignment to an object is not kept: nothing to own
                Ok(if t.is_heap() { Flow::Unit } else { Flow::Value(t) })
            }
            ExprKind::Index { recv, args } => {
                // the interpreter evaluates the value first, then the array and the index
                let item = self.value(value, None)?;
                let elem = self.element_target(recv, args)?;
                self.store_elem(recv, elem, item)?;
                self.typed.mutates = true;
                Ok(Flow::Unit)
            }
            ExprKind::Call { .. } => Err("assigns an attribute".into()),
            _ => Err("assigns something other than a variable or an element".into()),
        }
    }

    pub(super) fn op_assign(&mut self, op: BinOp, target: &Expr, value: &Expr) -> Result<Flow, Reject> {
        if matches!(op, BinOp::And | BinOp::Or) {
            return Err("uses `||=` or `&&=`".into());
        }
        match &target.kind {
            ExprKind::Var(name) => {
                let current = self.read(name)?;
                self.typed.types.insert(target as *const Expr, Flow::Value(current));
                let rhs = self.value(value, None)?;
                let t = self.binary_types(op, current, rhs)?;
                self.define(name, t)?;
                Ok(if t.is_heap() { Flow::Unit } else { Flow::Value(t) })
            }
            ExprKind::Index { recv, args } => {
                // the interpreter evaluates the target twice: only pure targets give the same result once
                if !matches!(recv.kind, ExprKind::Var(_)) || !args.iter().all(pure) {
                    return Err("updates an element whose array or index is computed".into());
                }
                let elem = self.element_target(recv, args)?;
                let Some(current) = elem.ty() else { return Err("updates an element of `[]`".into()) };
                self.typed.types.insert(target as *const Expr, Flow::Value(current));
                let rhs = self.value(value, None)?;
                let t = self.binary_types(op, current, rhs)?;
                self.store_elem(recv, elem, t)?;
                self.typed.mutates = true;
                Ok(Flow::Unit)
            }
            _ => Err("updates something other than a variable or an element".into()),
        }
    }

    /// `recv[index]` as an assignment target: element type of the array.
    fn element_target(&mut self, recv: &Expr, args: &[Expr]) -> Result<Elem, Reject> {
        let t = self.value(recv, None)?;
        let Ty::Array(elem) = t else { return Err(format!("assigns an element of a `{}`", self.show(t))) };
        let [index] = args else { return Err("indexes with several values".into()) };
        match self.value(index, None)? {
            Ty::Int => Ok(elem),
            t => Err(format!("indexes an array with a `{}`", self.show(t))),
        }
    }

    pub(super) fn binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> Result<Flow, Reject> {
        let l = self.value(lhs, None)?;
        if let (BinOp::Shl, Ty::Array(elem)) = (op, l) {
            let item = self.value(rhs, elem.ty())?;
            self.store_elem(lhs, elem, item)?;
            self.typed.mutates = true;
            // natively, `<<` is a statement: its value (the array) is not kept
            return Ok(Flow::Unit);
        }
        let r = self.value(rhs, matches!(l, Ty::Array(_)).then_some(l))?;
        Ok(Flow::Value(self.binary_types(op, l, r)?))
    }

    /// Result type of `l op r`, exactly as the interpreter computes it.
    pub(super) fn binary_types(&self, op: BinOp, l: Ty, r: Ty) -> Result<Ty, Reject> {
        use BinOp::*;
        Ok(match (op, l, r) {
            (Add | Sub | Mul | Div | Rem, Ty::Int, Ty::Int) => Ty::Int,
            (Rem, _, _) if l.is_numeric() && r.is_numeric() => return Err("uses `%` on a `Float`".into()),
            (Add | Sub | Mul | Div, _, _) if l.is_numeric() && r.is_numeric() => Ty::Float,
            (Lt | Le | Gt | Ge, _, _) if l.is_numeric() && r.is_numeric() => Ty::Bool,
            (Eq | NotEq, _, _) if l.is_numeric() && r.is_numeric() => Ty::Bool,
            (Cmp, _, _) if l.is_numeric() && r.is_numeric() => Ty::Int,
            (Eq | NotEq, Ty::Bool, Ty::Bool) => Ty::Bool,
            (And | Or, Ty::Bool, Ty::Bool) => Ty::Bool,
            (BitAnd | BitOr | BitXor, Ty::Int, Ty::Int) => Ty::Int,
            (Add, Ty::Str, Ty::Str) => Ty::Str,
            (Mul, Ty::Str, Ty::Int) => Ty::Str,
            (Eq | NotEq | Lt | Le | Gt | Ge, Ty::Str, Ty::Str) => Ty::Bool,
            (Cmp, Ty::Str, Ty::Str) => Ty::Int,
            (Add, Ty::Array(a), Ty::Array(b)) if a == b || b == Elem::Unknown => l,
            (Add, Ty::Array(Elem::Unknown), Ty::Array(_)) => r,
            (Pow, _, _) => return Err("uses `**`".into()),
            (Shl | Shr, _, _) => return Err("uses a bit shift".into()),
            (Eq | NotEq, _, _) if l.is_heap() && r.is_heap() => {
                return Err(format!("compares a `{}` and a `{}`", self.show(l), self.show(r)));
            }
            _ => return Err(format!("applies an operator to `{}` and `{}`", self.show(l), self.show(r))),
        })
    }

    pub(super) fn unary(&mut self, op: UnOp, e: &Expr) -> Result<Ty, Reject> {
        let t = self.value(e, None)?;
        match (op, t) {
            (UnOp::Neg, Ty::Int | Ty::Float) | (UnOp::Not, Ty::Bool) => Ok(t),
            (UnOp::Not, _) => Err(format!("applies `!` to a `{}`", self.show(t))),
            (UnOp::Neg, _) => Err(format!("negates a `{}`", self.show(t))),
        }
    }
}

/// No call and no assignment: evaluating it twice gives the same result.
fn pure(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Var(_) => true,
        ExprKind::Unary { expr, .. } => pure(expr),
        ExprKind::Binary { lhs, rhs, .. } => pure(lhs) && pure(rhs),
        _ => false,
    }
}
