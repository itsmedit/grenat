//! Strings: literals, interpolation, operators and methods, through the runtime.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{InstBuilder, Value, types};
use grenat_ast::{BinOp, Expr, StrSeg};
use grenat_runtime::layout;

use super::{Held, Translator};
use crate::infer::{Method, StrOp};
use crate::runtime::Rt;
use crate::ty::Ty;

impl Translator<'_, '_> {
    /// Address and length of the bytes of a literal (data of the module).
    fn literal(&mut self, text: &str) -> (Value, Value) {
        let data = self.env.literals[text];
        (self.b.ins().symbol_value(types::I64, data), self.b.ins().iconst(types::I64, text.len() as i64))
    }

    /// A new string holding `text`.
    fn new_string(&mut self, text: &str) -> Value {
        let (ptr, len) = self.literal(text);
        self.runtime(Rt::StrFrom, &[ptr, len])[0]
    }

    /// `"a #{b} c"`: built in place, piece by piece, as the interpreter displays them.
    pub(super) fn string(&mut self, segs: &[StrSeg]) -> Held {
        let (first, rest) = match segs.split_first() {
            Some((StrSeg::Lit(text), rest)) => (text.as_str(), rest),
            _ => ("", segs),
        };
        let s = self.new_string(first);
        self.hold(Held::object(s, Ty::Str));
        for (i, seg) in rest.iter().enumerate() {
            match seg {
                StrSeg::Lit(text) => {
                    let (ptr, len) = self.literal(text);
                    self.runtime(Rt::StrPushBytes, &[s, ptr, len]);
                }
                StrSeg::Interp(e) => {
                    let later: Vec<&Expr> = rest[i + 1..]
                        .iter()
                        .filter_map(|s| match s {
                            StrSeg::Interp(e) => Some(e),
                            StrSeg::Lit(_) => None,
                        })
                        .collect();
                    let piece = self.operand(e, &later);
                    self.push_display(s, piece);
                }
            }
        }
        self.take(s);
        Held::object(s, Ty::Str)
    }

    /// Appends the text of `piece` (`to_s`) to the string `s` being built.
    fn push_display(&mut self, s: Value, piece: Held) {
        match piece.ty {
            Ty::Int => {
                self.runtime(Rt::StrPushInt, &[s, piece.value]);
            }
            Ty::Float => {
                self.runtime(Rt::StrPushFloat, &[s, piece.value]);
            }
            Ty::Bool => {
                let b = self.b.ins().uextend(types::I64, piece.value);
                self.runtime(Rt::StrPushBool, &[s, b]);
            }
            Ty::Str => {
                self.runtime(Rt::StrPushStr, &[s, piece.value]);
                self.release(piece);
            }
            other => unreachable!("interpolation of {other:?} rejected by infer"),
        }
    }

    /// `x.to_s` for a number or a boolean.
    pub(super) fn display(&mut self, piece: Held) -> Held {
        let s = self.new_string("");
        self.push_display(s, piece);
        Held::object(s, Ty::Str)
    }

    /// `l op r` with a string on the left.
    pub(super) fn string_binary(&mut self, op: BinOp, l: Held, r: Held) -> Held {
        match op {
            BinOp::Add => {
                // a string the translator owns is given up: appended in place when unique
                let s = if l.owned {
                    self.take(l.value);
                    self.runtime(Rt::StrAddOwned, &[l.value, r.value])[0]
                } else {
                    self.runtime(Rt::StrConcat, &[l.value, r.value])[0]
                };
                self.hold(Held::object(s, Ty::Str));
                self.release(r);
                self.take(s);
                Held::object(s, Ty::Str)
            }
            BinOp::Mul => {
                let s = self.runtime(Rt::StrRepeat, &[l.value, r.value])[0];
                // a negative count is a `TypeError` in the interpreter
                self.deopt_if_null(s);
                self.release(l);
                Held::object(s, Ty::Str)
            }
            BinOp::Eq | BinOp::NotEq => {
                let eq = self.runtime_bool(Rt::StrEq, &[l.value, r.value]);
                self.release(l);
                self.release(r);
                let v = if op == BinOp::Eq { eq } else { self.b.ins().icmp_imm_s(IntCC::Equal, eq, 0) };
                Held::scalar(v, Ty::Bool)
            }
            _ => {
                let c = self.runtime(Rt::StrCmp, &[l.value, r.value])[0];
                self.release(l);
                self.release(r);
                let cc = match op {
                    BinOp::Lt => IntCC::SignedLessThan,
                    BinOp::Le => IntCC::SignedLessThanOrEqual,
                    BinOp::Gt => IntCC::SignedGreaterThan,
                    BinOp::Ge => IntCC::SignedGreaterThanOrEqual,
                    BinOp::Cmp => return Held::scalar(c, Ty::Int),
                    other => unreachable!("checked by infer: {other:?}"),
                };
                Held::scalar(self.b.ins().icmp_imm_s(cc, c, 0), Ty::Bool)
            }
        }
    }

    /// A method of a string (`recv`), with its argument if any.
    pub(super) fn string_method(&mut self, method: Method, recv: Held, arg: Option<Held>) -> Held {
        let s = recv.value;
        let result = match method {
            Method::Same => {
                // the receiver itself: its reference becomes the result's
                let v = self.consume(recv);
                return Held::object(v, Ty::Str);
            }
            Method::StrLength => Held::scalar(self.runtime(Rt::StrLength, &[s])[0], Ty::Int),
            Method::StrEmpty => {
                let len = self.load(types::I64, s, layout::LEN);
                Held::scalar(self.b.ins().icmp_imm_s(IntCC::Equal, len, 0), Ty::Bool)
            }
            Method::Str(op) => {
                let rt = match op {
                    StrOp::Upcase => Rt::StrUpcase,
                    StrOp::Downcase => Rt::StrDowncase,
                    StrOp::Reverse => Rt::StrReverse,
                    StrOp::Strip => Rt::StrStrip,
                    StrOp::Includes => Rt::StrIncludes,
                    StrOp::StartsWith => Rt::StrStartsWith,
                    StrOp::EndsWith => Rt::StrEndsWith,
                    StrOp::ToI => Rt::StrToI,
                    StrOp::ToF => Rt::StrToF,
                };
                match (op, arg) {
                    (_, Some(arg)) => {
                        let v = self.runtime_bool(rt, &[s, arg.value]);
                        self.release(arg);
                        Held::scalar(v, Ty::Bool)
                    }
                    (StrOp::ToI, None) => Held::scalar(self.runtime(rt, &[s])[0], Ty::Int),
                    (StrOp::ToF, None) => Held::scalar(self.runtime(rt, &[s])[0], Ty::Float),
                    (_, None) => Held::object(self.runtime(rt, &[s])[0], Ty::Str),
                }
            }
            other => unreachable!("not a string method: {other:?}"),
        };
        // the result is not registered yet: releasing the receiver cannot fail
        self.release(recv);
        result
    }

    /// `s[i]`: the character, or deoptimization when out of range (`nil`).
    pub(super) fn char_at(&mut self, recv: Held, index: Value) -> Held {
        let c = self.runtime(Rt::StrCharAt, &[recv.value, index])[0];
        self.deopt_if_null(c);
        self.release(recv);
        Held::object(c, Ty::Str)
    }
}
