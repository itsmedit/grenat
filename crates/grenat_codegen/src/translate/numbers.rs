//! Numbers and booleans: operators and methods.
//!
//! Checked integer arithmetic, division rounded toward negative infinity and
//! remainder with the sign of the divisor (as in Ruby), as the interpreter.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{InstBuilder, Value, types};
use grenat_ast::{BinOp, UnOp};

use super::Translator;
use crate::abi::Trap;
use crate::infer::Method;
use crate::ty::Ty;

impl Translator<'_, '_> {
    pub(super) fn unary(&mut self, op: UnOp, v: Value, ty: Ty) -> Value {
        match (op, ty) {
            (UnOp::Neg, Ty::Int) => {
                let is_min = self.b.ins().icmp_imm_s(IntCC::Equal, v, i64::MIN);
                self.trap_if(is_min, Trap::Overflow as i64);
                self.b.ins().ineg(v)
            }
            (UnOp::Neg, _) => self.b.ins().fneg(v),
            (UnOp::Not, _) => self.b.ins().icmp_imm_s(IntCC::Equal, v, 0),
        }
    }

    pub(super) fn as_float(&mut self, value: Value, ty: Ty) -> Value {
        match ty {
            Ty::Int => self.b.ins().fcvt_from_sint(types::F64, value),
            _ => value,
        }
    }

    /// `l op r` on numbers or booleans.
    pub(super) fn scalar_binary(&mut self, op: BinOp, l: Value, lt: Ty, r: Value, rt: Ty) -> Value {
        match (lt, rt) {
            (Ty::Int, Ty::Int) => self.int_binary(op, l, r),
            (Ty::Bool, Ty::Bool) => match op {
                BinOp::Eq => self.b.ins().icmp(IntCC::Equal, l, r),
                BinOp::NotEq => self.b.ins().icmp(IntCC::NotEqual, l, r),
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
                self.trap_if(overflow, Trap::Overflow as i64);
                v
            }
            Div => {
                self.check_divisor(r);
                // i64::MIN / -1 does not fit
                let is_min = self.b.ins().icmp_imm_s(IntCC::Equal, l, i64::MIN);
                let minus_one = self.b.ins().icmp_imm_s(IntCC::Equal, r, -1);
                let overflow = self.b.ins().band(is_min, minus_one);
                self.trap_if(overflow, Trap::Overflow as i64);
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
        self.trap_if(zero, Trap::DivisionByZero as i64);
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
    pub(super) fn ordering(&mut self, lt: Value, gt: Value) -> Value {
        let minus_one = self.b.ins().iconst(types::I64, -1);
        let one = self.b.ins().iconst(types::I64, 1);
        let zero = self.b.ins().iconst(types::I64, 0);
        let greater = self.b.ins().select(gt, one, zero);
        self.b.ins().select(lt, minus_one, greater)
    }

    /// A method of a number, on value `v` of type `ty`.
    pub(super) fn number_method(&mut self, method: Method, v: Value, ty: Ty) -> Value {
        match method {
            Method::Same => v,
            Method::IntToF => self.as_float(v, Ty::Int),
            Method::FloatToI => self.b.ins().fcvt_to_sint_sat(types::I64, v),
            Method::IntAbs => {
                let is_min = self.b.ins().icmp_imm_s(IntCC::Equal, v, i64::MIN);
                self.trap_if(is_min, Trap::Overflow as i64);
                self.b.ins().iabs(v)
            }
            Method::FloatAbs => self.b.ins().fabs(v),
            Method::IntZero => self.b.ins().icmp_imm_s(IntCC::Equal, v, 0),
            Method::FloatZero => {
                let zero = self.b.ins().f64const(0.0);
                self.b.ins().fcmp(FloatCC::Equal, v, zero)
            }
            Method::Even | Method::Odd => {
                let bit = self.b.ins().band_imm_s(v, 1);
                let cc = if method == Method::Even { IntCC::Equal } else { IntCC::NotEqual };
                self.b.ins().icmp_imm_s(cc, bit, 0)
            }
            // `f.floor() as i64`: saturating, NaN to 0
            Method::Floor => {
                let f = self.b.ins().floor(v);
                self.b.ins().fcvt_to_sint_sat(types::I64, f)
            }
            Method::Ceil => {
                let f = self.b.ins().ceil(v);
                self.b.ins().fcvt_to_sint_sat(types::I64, f)
            }
            Method::Sqrt => {
                let f = self.as_float(v, ty);
                self.b.ins().sqrt(f)
            }
            other => unreachable!("not a number method: {other:?}"),
        }
    }
}
