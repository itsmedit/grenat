//! Calls: native functions, methods, indexing, operators.

use cranelift_codegen::ir::{InstBuilder, Value};
use grenat_ast::{Arg, BinOp, Block, Expr, ExprKind};

use super::{Held, Translator};
use crate::infer::{Flow, Method};
use crate::ty::Ty;

fn positional(args: &[Arg]) -> Vec<&Expr> {
    args.iter()
        .map(|a| match a {
            Arg::Pos(e) => e,
            _ => unreachable!("rejected by infer"),
        })
        .collect()
}

/// Type of `l op r` on numbers or booleans, as [`infer`](crate::infer) computes it.
fn scalar_result(op: BinOp, l: Ty, r: Ty) -> Ty {
    use BinOp::*;
    match op {
        Lt | Le | Gt | Ge | Eq | NotEq | And | Or => Ty::Bool,
        Cmp | Rem | BitAnd | BitOr | BitXor => Ty::Int,
        _ if l == Ty::Int && r == Ty::Int => Ty::Int,
        _ => Ty::Float,
    }
}

impl Translator<'_, '_> {
    pub(super) fn call(&mut self, e: &Expr, recv: Option<&Expr>, args: &[Arg], block: Option<&Block>) -> Option<Held> {
        if self.typed.constructs.contains_key(&(e as *const Expr)) {
            return Some(self.construct(e, args));
        }
        let Some(recv) = recv else {
            return match self.typed.methods.get(&(e as *const Expr)) {
                Some(Method::Exit) => {
                    self.exit(args.first().map(|a| match a {
                        Arg::Pos(e) => e,
                        _ => unreachable!("rejected by infer"),
                    }));
                    None
                }
                Some(&method) => {
                    self.write(method, &positional(args));
                    None
                }
                None => self.native_call(e, args),
            };
        };
        let method = self.typed.method(e);
        let args = positional(args);
        match method {
            Method::Sqrt => {
                let arg = self.scalar(args[0]);
                let ty = self.typed.ty(args[0]);
                Some(Held::scalar(self.number_method(method, arg, ty), Ty::Float))
            }
            Method::Push => {
                self.push(recv, args[0]);
                None
            }
            Method::Times | Method::Upto => {
                let from = self.scalar(recv);
                let (from, to, inclusive) = match args.first() {
                    Some(end) => (from, self.scalar(end), true),
                    None => (self.b.ins().iconst(cranelift_codegen::ir::types::I64, 0), from, false),
                };
                self.count(e, from, to, inclusive, block.expect("a block"));
                None
            }
            Method::Each | Method::EachWithIndex => {
                self.each(e, recv, block.expect("a block"), method == Method::EachWithIndex);
                None
            }
            _ => {
                let held = self.operand(recv, &args);
                let arg = args.first().map(|a| self.operand(a, &[]));
                Some(self.method(e, method, held, arg))
            }
        }
    }

    /// A method without a block on an evaluated receiver.
    fn method(&mut self, e: &Expr, method: Method, recv: Held, arg: Option<Held>) -> Held {
        match (recv.ty, method) {
            (Ty::Int | Ty::Float | Ty::Bool, Method::ToS) => self.display(recv),
            (Ty::Int | Ty::Float, _) => {
                let v = self.number_method(method, recv.value, recv.ty);
                Held::scalar(v, self.typed.ty(e))
            }
            (Ty::Str, _) => self.string_method(method, recv, arg),
            (Ty::Array(_), _) => self.array_method(method, recv),
            (Ty::Struct(_), Method::Field(index)) => self.field(recv, index),
            (ty, method) => unreachable!("checked by infer: {ty:?}.{method:?}"),
        }
    }

    /// `recv[index]`
    pub(super) fn index(&mut self, e: &Expr, recv: &Expr, index: &Expr) -> Held {
        let held = self.operand(recv, &[index]);
        let i = self.scalar(index);
        match self.typed.method(e) {
            Method::At => {
                let element = self.element(held, i, "an index out of range (`nil`)");
                self.release(held);
                element
            }
            Method::CharAt => self.char_at(held, i),
            other => unreachable!("not an indexing: {other:?}"),
        }
    }

    /// A call to another compiled function: arguments are handed over (the
    /// callee owns them), a failure of the callee propagates.
    fn native_call(&mut self, e: &Expr, args: &[Arg]) -> Option<Held> {
        let ExprKind::Call { name, .. } = &e.kind else { unreachable!("a call") };
        let func = self.env.callees[name.name.as_str()];
        let exprs = positional(args);
        let held: Vec<Held> = exprs.iter().enumerate().map(|(i, a)| self.operand(a, &exprs[i + 1..])).collect();
        let mut values: Vec<Value> = held.into_iter().map(|h| self.consume(h)).collect();
        let depth = self.b.ins().iadd_imm_s(self.depth, 1);
        values.extend([depth, self.ctx]);
        let call = self.b.ins().call(func, &values);
        let (result, status) = (self.b.inst_results(call)[0], self.b.inst_results(call)[1]);
        self.propagate_trap(status);
        match self.typed.flow(e) {
            Flow::Value(ty) => Some(Held { value: result, ty, owned: ty.is_heap() }),
            _ => None,
        }
    }

    /// `lhs op rhs` (except `&&`/`||`).
    pub(super) fn binary(&mut self, e: &Expr, op: BinOp, lhs: &Expr, rhs: &Expr) -> Option<Held> {
        if op == BinOp::Shl && matches!(self.typed.ty(lhs), Ty::Array(_)) {
            self.push(lhs, rhs);
            return None;
        }
        let l = self.operand(lhs, &[rhs]);
        let r = self.operand(rhs, &[]);
        let result = self.binary_held(op, l, r);
        debug_assert_eq!(result.ty, self.typed.ty(e));
        Some(result)
    }

    /// `l op r` on evaluated operands, which it consumes.
    pub(super) fn binary_held(&mut self, op: BinOp, l: Held, r: Held) -> Held {
        match (l.ty, r.ty) {
            (Ty::Str, _) => self.string_binary(op, l, r),
            (Ty::Array(_), _) => self.array_concat(l, r),
            _ => {
                let v = self.scalar_binary(op, l.value, l.ty, r.value, r.ty);
                Held::scalar(v, scalar_result(op, l.ty, r.ty))
            }
        }
    }
}
