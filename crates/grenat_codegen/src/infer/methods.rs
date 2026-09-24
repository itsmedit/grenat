//! Methods, indexing and field reads of native values.
//!
//! Each supported call is resolved once here into a [`Method`], which the
//! translator implements: the two never disagree on what a call means.

use grenat_ast::{Arg, Block, Expr, ExprKind};

use super::{Flow, Infer, Reject, positional};
use crate::ty::{Elem, Ty};

/// A method (or indexing, or field read) that native code implements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Method {
    /// `to_i` on an `Int`, `to_f` on a `Float`, `to_s` on a `String`.
    Same,
    IntToF,
    /// Saturating, `NaN` giving 0: Rust's `as i64`, as in the interpreter.
    FloatToI,
    IntAbs,
    FloatAbs,
    IntZero,
    FloatZero,
    Even,
    Odd,
    Floor,
    Ceil,
    /// `Math.sqrt(x)`
    Sqrt,
    /// `to_s` of a number or a `Bool`.
    ToS,
    StrLength,
    StrEmpty,
    Str(StrOp),
    /// `s[i]`
    CharAt,
    Length,
    Empty,
    First,
    Last,
    Pop,
    Push,
    Sum,
    /// `dup`, `to_a`
    Copy,
    /// `xs[i]`
    At,
    /// `n.times do |i|`
    Times,
    /// `a.upto(b) do |i|`
    Upto,
    /// `xs.each do |x|`
    Each,
    /// `xs.each_with_index do |x, i|`
    EachWithIndex,
    /// Field of a struct, by index.
    Field(usize),
    /// `puts x, …` (standalone programs).
    Puts,
    /// `print x, …`
    Print,
    /// `p x, …`
    Inspect,
    /// `exit` / `exit code`
    Exit,
}

/// String methods implemented by the runtime.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StrOp {
    Upcase,
    Downcase,
    Reverse,
    Strip,
    Includes,
    StartsWith,
    EndsWith,
    ToI,
    ToF,
}

impl Infer<'_, '_> {
    pub(super) fn method(
        &mut self,
        call: &Expr,
        recv: &Expr,
        name: &str,
        args: &[Arg],
        block: Option<&Block>,
    ) -> Result<Flow, Reject> {
        let t = self.value(recv, None)?;
        let (method, flow) = match block {
            Some(block) => (self.block_method(call, t, name, args, block)?, Flow::Unit),
            None => {
                let args = positional(args, name)?;
                let (method, ty) = self.plain_method(recv, t, name, &args)?;
                (method, ty.map_or(Flow::Unit, Flow::Value))
            }
        };
        self.typed.methods.insert(call as *const Expr, method);
        Ok(flow)
    }

    /// Resolves `recv.name(args)`; the type is `None` for a statement.
    fn plain_method(&mut self, recv: &Expr, t: Ty, name: &str, args: &[&Expr]) -> Result<(Method, Option<Ty>), Reject> {
        use Method::*;
        if let [arg] = args {
            return self.method_with_arg(recv, t, name, arg);
        }
        if !args.is_empty() {
            return Err(format!("calls `{name}` with {} arguments", args.len()));
        }
        let element = |elem: Elem| elem.ty().ok_or_else(|| format!("calls `{name}` on `[]`"));
        Ok(match (t, name) {
            (Ty::Int, "to_i" | "round" | "floor" | "ceil") | (Ty::Float, "to_f") | (Ty::Str, "to_s") => (Same, Some(t)),
            (Ty::Int, "to_f") => (IntToF, Some(Ty::Float)),
            (Ty::Float, "to_i") => (FloatToI, Some(Ty::Int)),
            (Ty::Int, "abs") => (IntAbs, Some(Ty::Int)),
            (Ty::Float, "abs") => (FloatAbs, Some(Ty::Float)),
            (Ty::Int, "zero?") => (IntZero, Some(Ty::Bool)),
            (Ty::Float, "zero?") => (FloatZero, Some(Ty::Bool)),
            (Ty::Int, "even?") => (Even, Some(Ty::Bool)),
            (Ty::Int, "odd?") => (Odd, Some(Ty::Bool)),
            (Ty::Float, "floor") => (Floor, Some(Ty::Int)),
            (Ty::Float, "ceil") => (Ceil, Some(Ty::Int)),
            (Ty::Int | Ty::Float | Ty::Bool, "to_s") => (ToS, Some(Ty::Str)),
            (Ty::Str, "length" | "size") => (StrLength, Some(Ty::Int)),
            (Ty::Str, "empty?") => (StrEmpty, Some(Ty::Bool)),
            (Ty::Str, "upcase") => (Str(StrOp::Upcase), Some(Ty::Str)),
            (Ty::Str, "downcase") => (Str(StrOp::Downcase), Some(Ty::Str)),
            (Ty::Str, "reverse") => (Str(StrOp::Reverse), Some(Ty::Str)),
            (Ty::Str, "strip") => (Str(StrOp::Strip), Some(Ty::Str)),
            (Ty::Str, "to_i") => (Str(StrOp::ToI), Some(Ty::Int)),
            (Ty::Str, "to_f") => (Str(StrOp::ToF), Some(Ty::Float)),
            (Ty::Array(_), "length" | "size" | "count") => (Length, Some(Ty::Int)),
            (Ty::Array(_), "empty?") => (Empty, Some(Ty::Bool)),
            (Ty::Array(elem), "first") => (First, Some(element(elem)?)),
            (Ty::Array(elem), "last") => (Last, Some(element(elem)?)),
            (Ty::Array(elem), "pop") => {
                self.typed.mutates = true;
                (Pop, Some(element(elem)?))
            }
            (Ty::Array(Elem::Int), "sum") => (Sum, Some(Ty::Int)),
            (Ty::Array(Elem::Float), "sum") => (Sum, Some(Ty::Float)),
            (Ty::Array(_), "dup" | "to_a") => (Copy, Some(t)),
            (Ty::Struct(id), _) => {
                let def = self.structs.get(id);
                match def.field(name) {
                    Some(i) if !def.has_method(name) => (Field(i), Some(def.fields[i].1)),
                    _ => return Err(format!("calls `{}.{name}`", def.name)),
                }
            }
            _ => return Err(format!("calls `{}.{name}`", self.show(t))),
        })
    }

    fn method_with_arg(&mut self, recv: &Expr, t: Ty, name: &str, arg: &Expr) -> Result<(Method, Option<Ty>), Reject> {
        let op = match (t, name) {
            (Ty::Str, "include?") => StrOp::Includes,
            (Ty::Str, "start_with?") => StrOp::StartsWith,
            (Ty::Str, "end_with?") => StrOp::EndsWith,
            (Ty::Array(elem), "push" | "append") => {
                let item = self.value(arg, elem.ty())?;
                self.store_elem(recv, elem, item)?;
                self.typed.mutates = true;
                return Ok((Method::Push, None));
            }
            _ => return Err(format!("calls `{}.{name}` with an argument", self.show(t))),
        };
        match self.value(arg, None)? {
            Ty::Str => Ok((Method::Str(op), Some(Ty::Bool))),
            other => Err(format!("calls `String.{name}` with a `{}`", self.show(other))),
        }
    }

    /// Loops over a block, compiled inline.
    fn block_method(&mut self, call: &Expr, t: Ty, name: &str, args: &[Arg], block: &Block) -> Result<Method, Reject> {
        let args = positional(args, name)?;
        let (method, params) = match (t, name, args.as_slice()) {
            (Ty::Int, "times", []) => (Method::Times, vec![Ty::Int]),
            (Ty::Int, "upto", [end]) => {
                if self.value(end, None)? != Ty::Int {
                    return Err("calls `upto` with a non-`Int` bound".into());
                }
                (Method::Upto, vec![Ty::Int])
            }
            (Ty::Array(elem), "each", []) => (Method::Each, vec![elem.ty().ok_or("iterates over `[]`")?]),
            (Ty::Array(elem), "each_with_index", []) => {
                (Method::EachWithIndex, vec![elem.ty().ok_or("iterates over `[]`")?, Ty::Int])
            }
            _ => return Err(format!("calls `{}.{name}` with a block", self.show(t))),
        };
        self.block(call, block, &params)?;
        Ok(method)
    }

    /// `recv[index]`
    pub(super) fn index(&mut self, e: &Expr, recv: &Expr, args: &[Expr]) -> Result<Ty, Reject> {
        let t = self.value(recv, None)?;
        let [index] = args else { return Err("indexes with several values".into()) };
        let i = self.value(index, None)?;
        if i != Ty::Int {
            return Err(format!("indexes with a `{}`", self.show(i)));
        }
        let (method, ty) = match t {
            Ty::Array(elem) => (Method::At, elem.ty().ok_or("indexes `[]`")?),
            Ty::Str => (Method::CharAt, Ty::Str),
            other => return Err(format!("indexes a `{}`", self.show(other))),
        };
        self.typed.methods.insert(e as *const Expr, method);
        Ok(ty)
    }

    /// `Math.sqrt(x)`
    pub(super) fn sqrt(&mut self, call: &Expr, recv: &Expr, args: &[Arg]) -> Result<Flow, Reject> {
        let [Arg::Pos(arg)] = args else { return Err("calls `Math.sqrt` with bad arguments".into()) };
        let t = self.value(arg, None)?;
        if !t.is_numeric() {
            return Err(format!("takes the square root of a `{}`", self.show(t)));
        }
        self.typed.types.insert(recv as *const Expr, Flow::Unit);
        self.typed.methods.insert(call as *const Expr, Method::Sqrt);
        Ok(Flow::Value(Ty::Float))
    }
}

/// `Math` or a struct name used as a receiver.
pub(super) fn constant(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Const(path) if path.len() == 1 => Some(&path[0].name),
        _ => None,
    }
}
