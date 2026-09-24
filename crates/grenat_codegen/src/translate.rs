//! Translation of a typed function body into Cranelift IR.
//!
//! Semantics match the interpreter exactly: checked integer arithmetic,
//! division rounded toward negative infinity and remainder with the sign of
//! the divisor (as in Ruby), and the same recursion limit. Errors are not
//! unwound: they jump to the single exit block, which returns a zero value
//! and the [`Trap`] code as status (see [`abi`](crate::abi)).

use std::collections::HashMap;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{Block, BlockArg, FuncRef, InstBuilder, Value, types};
use cranelift_frontend::{FunctionBuilder, Variable};
use grenat_ast::{Arg, BinOp, Expr, ExprKind, FnDef, UnOp};

use crate::abi::Trap;
use crate::infer::{Signature, Ty, Typed};
use crate::scalar::ScalarTy;

/// Compiled functions other functions may call from native code, by name.
pub(crate) type Callees<'a> = HashMap<&'a str, FuncRef>;

pub(crate) struct Translator<'a, 'b> {
    b: FunctionBuilder<'b>,
    typed: &'a Typed,
    callees: &'a Callees<'a>,
    vars: HashMap<String, (Variable, ScalarTy)>,
    /// Recursion depth of this call and its limit, passed in registers.
    depth: Value,
    limit: Value,
    exit: Block,
    ret: ScalarTy,
}

impl<'a, 'b> Translator<'a, 'b> {
    /// Emits the whole function: entry (depth check), body, exit block.
    /// Parameters: the scalars, then `depth` and `limit`; results: the value, then the status.
    pub fn function(
        mut b: FunctionBuilder<'b>,
        def: &FnDef,
        sig: &Signature,
        typed: &'a Typed,
        callees: &'a Callees<'a>,
    ) -> FunctionBuilder<'b> {
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let params = b.block_params(entry).to_vec();
        let (depth, limit) = (params[sig.params.len()], params[sig.params.len() + 1]);
        let exit = b.create_block();
        b.append_block_param(exit, sig.ret.clif());
        b.append_block_param(exit, types::I64);

        let mut t = Translator { b, typed, callees, vars: HashMap::new(), depth, limit, exit, ret: sig.ret };
        for ((param, ty), value) in def.params.iter().zip(&sig.params).zip(&params) {
            let var = t.b.declare_var(ty.clif());
            t.b.def_var(var, *value);
            t.vars.insert(param.name.name.clone(), (var, *ty));
        }
        // every local gets a value up front; definite assignment guarantees it is never read
        for (name, ty) in &typed.locals {
            let var = t.b.declare_var(ty.clif());
            let zero = t.zero(*ty);
            t.b.def_var(var, zero);
            t.vars.insert(name.clone(), (var, *ty));
        }

        let too_deep = t.b.ins().icmp(IntCC::SignedGreaterThan, depth, limit);
        t.trap_if(too_deep, Trap::StackOverflow);

        match t.stmts(&def.body.stmts) {
            Some(value) => {
                let ok = t.b.ins().iconst(types::I64, 0);
                t.b.ins().jump(exit, &[BlockArg::Value(value), BlockArg::Value(ok)]);
            }
            None => {
                let ok = t.b.ins().iconst(types::I64, 0);
                t.jump_exit_zero(ok);
            }
        }

        t.b.switch_to_block(exit);
        let (result, status) = (t.b.block_params(exit)[0], t.b.block_params(exit)[1]);
        t.b.ins().return_(&[result, status]);
        t.b.seal_all_blocks();
        t.b
    }

    // ── Helpers ──────────────────────────────────────────────

    fn zero(&mut self, ty: ScalarTy) -> Value {
        match ty {
            ScalarTy::Float => self.b.ins().f64const(0.0),
            other => self.b.ins().iconst(other.clif(), 0),
        }
    }

    /// Leaves with a zero value and `status`.
    fn jump_exit_zero(&mut self, status: Value) {
        let zero = self.zero(self.ret);
        self.b.ins().jump(self.exit, &[BlockArg::Value(zero), BlockArg::Value(status)]);
    }

    /// Continues in a block no one jumps to (code after `return` or a trap).
    fn unreachable_block(&mut self) {
        let dead = self.b.create_block();
        self.b.switch_to_block(dead);
    }

    /// If `cond` holds: record `trap` and leave the function.
    fn trap_if(&mut self, cond: Value, trap: Trap) {
        let fail = self.b.create_block();
        let cont = self.b.create_block();
        self.b.ins().brif(cond, fail, &[], cont, &[]);
        self.b.switch_to_block(fail);
        let code = self.b.ins().iconst(types::I64, trap as i64);
        self.jump_exit_zero(code);
        self.b.switch_to_block(cont);
    }

    /// After a native call: leave at once with the callee's status if it failed.
    fn propagate_trap(&mut self, status: Value) {
        let failed = self.b.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        let fail = self.b.create_block();
        let cont = self.b.create_block();
        self.b.ins().brif(failed, fail, &[], cont, &[]);
        self.b.switch_to_block(fail);
        self.jump_exit_zero(status);
        self.b.switch_to_block(cont);
    }

    fn scalar(&self, e: &Expr) -> ScalarTy {
        match self.typed.ty(e) {
            Ty::Scalar(t) => t,
            other => unreachable!("value expected, typed {other:?}"),
        }
    }

    fn as_float(&mut self, value: Value, ty: ScalarTy) -> Value {
        match ty {
            ScalarTy::Int => self.b.ins().fcvt_from_sint(types::F64, value),
            _ => value,
        }
    }

    fn bool_const(&mut self, b: bool) -> Value {
        self.b.ins().iconst(types::I8, i64::from(b))
    }

    // ── Statements ───────────────────────────────────────────

    /// Value of the last statement, or `None` for a statement without value.
    fn stmts(&mut self, stmts: &[Expr]) -> Option<Value> {
        let mut last = None;
        for stmt in stmts {
            last = self.expr(stmt);
        }
        last
    }

    fn expr(&mut self, e: &Expr) -> Option<Value> {
        match &e.kind {
            ExprKind::Int(n) => Some(self.b.ins().iconst(types::I64, *n)),
            ExprKind::Float(f) => Some(self.b.ins().f64const(*f)),
            ExprKind::Bool(b) => Some(self.bool_const(*b)),
            ExprKind::Var(name) => {
                let (var, _) = self.vars[name];
                Some(self.b.use_var(var))
            }
            ExprKind::Assign { target, value } => {
                let ExprKind::Var(name) = &target.kind else { unreachable!("checked by infer") };
                let v = self.value(value);
                let (var, _) = self.vars[name];
                self.b.def_var(var, v);
                Some(v)
            }
            ExprKind::OpAssign { op, target, value } => {
                let ExprKind::Var(name) = &target.kind else { unreachable!("checked by infer") };
                let (var, ty) = self.vars[name];
                let current = self.b.use_var(var);
                let rhs = self.value(value);
                let rhs_ty = self.scalar(value);
                let v = self.binary(*op, current, ty, rhs, rhs_ty);
                self.b.def_var(var, v);
                Some(v)
            }
            ExprKind::Binary { op: BinOp::And, lhs, rhs } => Some(self.short_circuit(lhs, rhs, true)),
            ExprKind::Binary { op: BinOp::Or, lhs, rhs } => Some(self.short_circuit(lhs, rhs, false)),
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.value(lhs), self.value(rhs));
                let (lt, rt) = (self.scalar(lhs), self.scalar(rhs));
                Some(self.binary(*op, l, lt, r, rt))
            }
            ExprKind::Unary { op, expr } => {
                let v = self.value(expr);
                Some(match (op, self.scalar(expr)) {
                    (UnOp::Neg, ScalarTy::Int) => {
                        let is_min = self.b.ins().icmp_imm_s(IntCC::Equal, v, i64::MIN);
                        self.trap_if(is_min, Trap::Overflow);
                        self.b.ins().ineg(v)
                    }
                    (UnOp::Neg, _) => self.b.ins().fneg(v),
                    (UnOp::Not, _) => self.b.ins().icmp_imm_s(IntCC::Equal, v, 0),
                })
            }
            ExprKind::If { cond, then, else_ } => self.if_expr(e, cond, then, else_.as_deref()),
            ExprKind::While { cond, body } => {
                let header = self.b.create_block();
                let body_block = self.b.create_block();
                let after = self.b.create_block();
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let c = self.value(cond);
                self.b.ins().brif(c, body_block, &[], after, &[]);
                self.b.switch_to_block(body_block);
                self.stmts(body);
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(after);
                None
            }
            ExprKind::Return(Some(value)) => {
                let v = self.value(value);
                let ok = self.b.ins().iconst(types::I64, 0);
                self.b.ins().jump(self.exit, &[BlockArg::Value(v), BlockArg::Value(ok)]);
                self.unreachable_block();
                None
            }
            ExprKind::Call { recv: None, name, args, .. } => {
                let func = self.callees[name.name.as_str()];
                let mut values: Vec<Value> = args
                    .iter()
                    .map(|a| match a {
                        Arg::Pos(e) => self.value(e),
                        _ => unreachable!("checked by infer"),
                    })
                    .collect();
                let depth = self.b.ins().iadd_imm_s(self.depth, 1);
                values.extend([depth, self.limit]);
                let call = self.b.ins().call(func, &values);
                let (result, status) = (self.b.inst_results(call)[0], self.b.inst_results(call)[1]);
                self.propagate_trap(status);
                Some(result)
            }
            ExprKind::Call { recv: Some(recv), name, args, .. } => Some(self.method(recv, &name.name, args)),
            other => unreachable!("rejected by infer: {other:?}"),
        }
    }

    fn value(&mut self, e: &Expr) -> Value {
        self.expr(e).expect("value expected (checked by infer)")
    }

    fn if_expr(&mut self, e: &Expr, cond: &Expr, then: &[Expr], else_: Option<&[Expr]>) -> Option<Value> {
        let result = match self.typed.ty(e) {
            Ty::Scalar(t) => Some(t),
            _ => None,
        };
        let c = self.value(cond);
        let then_block = self.b.create_block();
        let else_block = self.b.create_block();
        let merge = self.b.create_block();
        if let Some(t) = result {
            self.b.append_block_param(merge, t.clif());
        }
        self.b.ins().brif(c, then_block, &[], else_block, &[]);
        for (block, stmts) in [(then_block, Some(then)), (else_block, else_)] {
            self.b.switch_to_block(block);
            let value = stmts.and_then(|s| self.stmts(s));
            match result {
                Some(t) => {
                    // a branch that returned is unreachable here: any value of the right type will do
                    let v = value.unwrap_or_else(|| self.zero(t));
                    self.b.ins().jump(merge, &[BlockArg::Value(v)]);
                }
                None => {
                    self.b.ins().jump(merge, &[]);
                }
            }
        }
        self.b.switch_to_block(merge);
        result.map(|_| self.b.block_params(merge)[0])
    }

    /// `a && b` / `a || b` on booleans: `b` only runs when it decides the result.
    fn short_circuit(&mut self, lhs: &Expr, rhs: &Expr, and: bool) -> Value {
        let l = self.value(lhs);
        let rhs_block = self.b.create_block();
        let merge = self.b.create_block();
        self.b.append_block_param(merge, types::I8);
        if and {
            self.b.ins().brif(l, rhs_block, &[], merge, &[BlockArg::Value(l)]);
        } else {
            self.b.ins().brif(l, merge, &[BlockArg::Value(l)], rhs_block, &[]);
        }
        self.b.switch_to_block(rhs_block);
        let r = self.value(rhs);
        self.b.ins().jump(merge, &[BlockArg::Value(r)]);
        self.b.switch_to_block(merge);
        self.b.block_params(merge)[0]
    }

    // ── Operators ────────────────────────────────────────────

    fn binary(&mut self, op: BinOp, l: Value, lt: ScalarTy, r: Value, rt: ScalarTy) -> Value {
        use BinOp::*;
        use ScalarTy::*;
        match (lt, rt) {
            (Int, Int) => self.int_binary(op, l, r),
            (Bool, Bool) => match op {
                Eq => self.b.ins().icmp(IntCC::Equal, l, r),
                NotEq => self.b.ins().icmp(IntCC::NotEqual, l, r),
                _ => unreachable!("checked by infer"),
            },
            _ => {
                let (l, r) = (self.as_float(l, lt), self.as_float(r, rt));
                self.float_binary(op, l, r)
            }
        }
    }

    fn int_binary(&mut self, op: BinOp, l: Value, r: Value) -> Value {
        use BinOp::*;
        match op {
            Add | Sub | Mul => {
                let (v, overflow) = match op {
                    Add => self.b.ins().sadd_overflow(l, r),
                    Sub => self.b.ins().ssub_overflow(l, r),
                    _ => self.b.ins().smul_overflow(l, r),
                };
                self.trap_if(overflow, Trap::Overflow);
                v
            }
            Div => {
                self.check_divisor(r);
                // i64::MIN / -1 does not fit
                let is_min = self.b.ins().icmp_imm_s(IntCC::Equal, l, i64::MIN);
                let minus_one = self.b.ins().icmp_imm_s(IntCC::Equal, r, -1);
                let overflow = self.b.ins().band(is_min, minus_one);
                self.trap_if(overflow, Trap::Overflow);
                let q = self.b.ins().sdiv(l, r);
                let rem = self.b.ins().srem(l, r);
                // round toward negative infinity when the signs differ and the division is inexact
                let inexact = self.b.ins().icmp_imm_s(IntCC::NotEqual, rem, 0);
                let signs = self.b.ins().bxor(l, r);
                let opposite = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, signs, 0);
                let adjust = self.b.ins().band(inexact, opposite);
                let q_minus_one = self.b.ins().iadd_imm_s(q, -1);
                self.b.ins().select(adjust, q_minus_one, q)
            }
            Rem => {
                self.check_divisor(r);
                // x % -1 is 0; dividing by 1 instead avoids the i64::MIN / -1 hardware trap
                let minus_one = self.b.ins().icmp_imm_s(IntCC::Equal, r, -1);
                let one = self.b.ins().iconst(types::I64, 1);
                let divisor = self.b.ins().select(minus_one, one, r);
                let rem = self.b.ins().srem(l, divisor);
                // result takes the sign of the divisor
                let nonzero = self.b.ins().icmp_imm_s(IntCC::NotEqual, rem, 0);
                let signs = self.b.ins().bxor(rem, r);
                let opposite = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, signs, 0);
                let adjust = self.b.ins().band(nonzero, opposite);
                let shifted = self.b.ins().iadd(rem, r);
                self.b.ins().select(adjust, shifted, rem)
            }
            Lt => self.b.ins().icmp(IntCC::SignedLessThan, l, r),
            Le => self.b.ins().icmp(IntCC::SignedLessThanOrEqual, l, r),
            Gt => self.b.ins().icmp(IntCC::SignedGreaterThan, l, r),
            Ge => self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, l, r),
            Eq => self.b.ins().icmp(IntCC::Equal, l, r),
            NotEq => self.b.ins().icmp(IntCC::NotEqual, l, r),
            Cmp => {
                let lt = self.b.ins().icmp(IntCC::SignedLessThan, l, r);
                let gt = self.b.ins().icmp(IntCC::SignedGreaterThan, l, r);
                self.ordering(lt, gt)
            }
            BitAnd => self.b.ins().band(l, r),
            BitOr => self.b.ins().bor(l, r),
            BitXor => self.b.ins().bxor(l, r),
            _ => unreachable!("checked by infer"),
        }
    }

    fn check_divisor(&mut self, r: Value) {
        let zero = self.b.ins().icmp_imm_s(IntCC::Equal, r, 0);
        self.trap_if(zero, Trap::DivisionByZero);
    }

    fn float_binary(&mut self, op: BinOp, l: Value, r: Value) -> Value {
        use BinOp::*;
        match op {
            Add => self.b.ins().fadd(l, r),
            Sub => self.b.ins().fsub(l, r),
            Mul => self.b.ins().fmul(l, r),
            Div => self.b.ins().fdiv(l, r),
            Lt => self.b.ins().fcmp(FloatCC::LessThan, l, r),
            Le => self.b.ins().fcmp(FloatCC::LessThanOrEqual, l, r),
            Gt => self.b.ins().fcmp(FloatCC::GreaterThan, l, r),
            Ge => self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, l, r),
            Eq => self.b.ins().fcmp(FloatCC::Equal, l, r),
            NotEq => self.b.ins().fcmp(FloatCC::NotEqual, l, r),
            // NaN compares as 0, like the interpreter's `partial_cmp(..).map_or(0, …)`
            Cmp => {
                let lt = self.b.ins().fcmp(FloatCC::LessThan, l, r);
                let gt = self.b.ins().fcmp(FloatCC::GreaterThan, l, r);
                self.ordering(lt, gt)
            }
            _ => unreachable!("checked by infer"),
        }
    }

    /// `<=>`: -1, 0 or 1.
    fn ordering(&mut self, lt: Value, gt: Value) -> Value {
        let minus_one = self.b.ins().iconst(types::I64, -1);
        let one = self.b.ins().iconst(types::I64, 1);
        let zero = self.b.ins().iconst(types::I64, 0);
        let greater = self.b.ins().select(gt, one, zero);
        self.b.ins().select(lt, minus_one, greater)
    }

    // ── Methods ──────────────────────────────────────────────

    fn method(&mut self, recv: &Expr, name: &str, args: &[Arg]) -> Value {
        if name == "sqrt" && matches!(recv.kind, ExprKind::Const(_)) {
            let [Arg::Pos(arg)] = args else { unreachable!("checked by infer") };
            let v = self.value(arg);
            let v = self.as_float(v, self.scalar(arg));
            return self.b.ins().sqrt(v);
        }
        let v = self.value(recv);
        match (self.scalar(recv), name) {
            (ScalarTy::Int, "to_i") | (ScalarTy::Float, "to_f") => v,
            (ScalarTy::Int, "to_f") => self.as_float(v, ScalarTy::Int),
            // saturating, NaN to 0: Rust's `as i64`, used by the interpreter
            (ScalarTy::Float, "to_i") => self.b.ins().fcvt_to_sint_sat(types::I64, v),
            (ScalarTy::Int, "abs") => {
                let is_min = self.b.ins().icmp_imm_s(IntCC::Equal, v, i64::MIN);
                self.trap_if(is_min, Trap::Overflow);
                self.b.ins().iabs(v)
            }
            (ScalarTy::Float, "abs") => self.b.ins().fabs(v),
            (ScalarTy::Int, "zero?") => self.b.ins().icmp_imm_s(IntCC::Equal, v, 0),
            (ScalarTy::Float, "zero?") => {
                let zero = self.b.ins().f64const(0.0);
                self.b.ins().fcmp(FloatCC::Equal, v, zero)
            }
            (ScalarTy::Int, "even?" | "odd?") => {
                let bit = self.b.ins().band_imm_s(v, 1);
                let cc = if name == "even?" { IntCC::Equal } else { IntCC::NotEqual };
                self.b.ins().icmp_imm_s(cc, bit, 0)
            }
            (ty, name) => unreachable!("checked by infer: {ty}.{name}"),
        }
    }
}
