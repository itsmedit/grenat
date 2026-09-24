//! Typing of a function body for native compilation.
//!
//! Only a numeric subset is compiled; anything else is rejected with a reason
//! and the function stays interpreted. Beyond types, this pass enforces
//! *definite assignment*: the interpreter raises `NameError` when a local is
//! read before being assigned, whereas native code would silently read zero,
//! so such functions must not be compiled.

use std::collections::{HashMap, HashSet};

use grenat_ast::{BinOp, Expr, ExprKind, FnDef, UnOp};

use crate::scalar::ScalarTy;

/// Native signature of a compilable function.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub params: Vec<ScalarTy>,
    pub ret: ScalarTy,
}

pub(crate) type Signatures<'p> = HashMap<&'p str, Signature>;

/// Static type of an expression.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Ty {
    Scalar(ScalarTy),
    /// No value (a loop, an `if` without `else` used as a statement).
    Unit,
    /// Control never continues past it (`return`).
    Never,
}

/// Result of typing a function body, consumed by the translator.
#[derive(Debug, Default)]
pub(crate) struct Typed {
    /// Locals other than parameters, in order of first assignment.
    pub locals: Vec<(String, ScalarTy)>,
    /// Type of every expression, keyed by address in the AST.
    pub types: HashMap<*const Expr, Ty>,
}

impl Typed {
    pub fn ty(&self, e: &Expr) -> Ty {
        self.types[&(e as *const Expr)]
    }
}

pub(crate) type Reject = String;

/// Types `def` against the signatures of every compilable function.
pub(crate) fn infer(def: &FnDef, sig: &Signature, sigs: &Signatures) -> Result<Typed, Reject> {
    if !def.body.rescues.is_empty() || def.body.ensure.is_some() {
        return Err("uses `rescue`/`ensure`".into());
    }
    let mut cx = Infer { sigs, ret: sig.ret, vars: HashMap::new(), assigned: HashSet::new(), typed: Typed::default() };
    for (param, ty) in def.params.iter().zip(&sig.params) {
        cx.vars.insert(param.name.name.clone(), *ty);
        cx.assigned.insert(param.name.name.clone());
    }
    let body = cx.stmts(&def.body.stmts)?;
    match body {
        Ty::Scalar(t) if t == sig.ret => {}
        Ty::Never => {}
        Ty::Scalar(t) => return Err(format!("returns `{t}` instead of `{}`", sig.ret)),
        Ty::Unit => return Err("its last statement has no value".into()),
    }
    Ok(cx.typed)
}

struct Infer<'a, 'p> {
    sigs: &'a Signatures<'p>,
    ret: ScalarTy,
    vars: HashMap<String, ScalarTy>,
    /// Locals certainly assigned at this point of the body.
    assigned: HashSet<String>,
    typed: Typed,
}

impl Infer<'_, '_> {
    fn stmts(&mut self, stmts: &[Expr]) -> Result<Ty, Reject> {
        let mut last = Ty::Unit;
        for stmt in stmts {
            if last == Ty::Never {
                return Err("has unreachable code after `return`".into());
            }
            last = self.expr(stmt)?;
        }
        Ok(last)
    }

    fn value(&mut self, e: &Expr) -> Result<ScalarTy, Reject> {
        match self.expr(e)? {
            Ty::Scalar(t) => Ok(t),
            _ => Err("uses a statement where a value is expected".into()),
        }
    }

    fn expr(&mut self, e: &Expr) -> Result<Ty, Reject> {
        let ty = self.expr_kind(e)?;
        self.typed.types.insert(e as *const Expr, ty);
        Ok(ty)
    }

    fn expr_kind(&mut self, e: &Expr) -> Result<Ty, Reject> {
        use ScalarTy::*;
        Ok(match &e.kind {
            ExprKind::Int(_) => Ty::Scalar(Int),
            ExprKind::Float(_) => Ty::Scalar(Float),
            ExprKind::Bool(_) => Ty::Scalar(Bool),
            ExprKind::Var(name) => Ty::Scalar(self.read(name)?),
            ExprKind::Assign { target, value } => {
                let ExprKind::Var(name) = &target.kind else {
                    return Err("assigns something other than a local variable".into());
                };
                let t = self.value(value)?;
                self.define(name, t)?;
                Ty::Scalar(t)
            }
            ExprKind::OpAssign { op, target, value } => {
                let ExprKind::Var(name) = &target.kind else {
                    return Err("assigns something other than a local variable".into());
                };
                let current = self.read(name)?;
                let rhs = self.value(value)?;
                if matches!(op, BinOp::And | BinOp::Or) {
                    return Err("uses `||=` or `&&=`".into());
                }
                let t = binary(*op, current, rhs)?;
                self.define(name, t)?;
                Ty::Scalar(t)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (l, r) = (self.value(lhs)?, self.value(rhs)?);
                Ty::Scalar(binary(*op, l, r)?)
            }
            ExprKind::Unary { op, expr } => {
                let t = self.value(expr)?;
                match (op, t) {
                    (UnOp::Neg, Int | Float) | (UnOp::Not, Bool) => Ty::Scalar(t),
                    (UnOp::Neg, _) => return Err("negates a `Bool`".into()),
                    (UnOp::Not, _) => return Err("applies `!` to a number".into()),
                }
            }
            ExprKind::If { cond, then, else_ } => self.if_expr(cond, then, else_.as_deref())?,
            ExprKind::While { cond, body } => {
                self.condition(cond)?;
                let before = self.assigned.clone();
                if self.stmts(body)? == Ty::Never {
                    return Err("returns from inside a loop".into());
                }
                // the loop may run zero times: nothing it assigns is certain afterwards
                self.assigned = before;
                Ty::Unit
            }
            ExprKind::Return(value) => {
                let Some(value) = value else { return Err("returns nothing".into()) };
                let t = self.value(value)?;
                if t != self.ret {
                    return Err(format!("returns `{t}` instead of `{}`", self.ret));
                }
                Ty::Never
            }
            ExprKind::Call { recv: None, name, args, block: None, .. } => {
                let Some(sig) = self.sigs.get(name.name.as_str()) else {
                    return Err(format!("calls `{}`, which is not compiled", name.name));
                };
                let sig = sig.clone();
                if args.len() != sig.params.len() {
                    return Err(format!("calls `{}` with default or missing arguments", name.name));
                }
                for (arg, expected) in args.iter().zip(&sig.params) {
                    let grenat_ast::Arg::Pos(arg) = arg else {
                        return Err(format!("calls `{}` with named arguments", name.name));
                    };
                    // exact types only: the interpreter would compute with the argument's own type
                    let t = self.value(arg)?;
                    if t != *expected {
                        return Err(format!("passes `{t}` where `{}` expects `{expected}`", name.name));
                    }
                }
                Ty::Scalar(sig.ret)
            }
            ExprKind::Call { recv: Some(recv), name, args, block: None, safe: false, .. } => {
                self.method(recv, &name.name, args)?
            }
            other => return Err(format!("uses {}", describe(other))),
        })
    }

    fn if_expr(&mut self, cond: &Expr, then: &[Expr], else_: Option<&[Expr]>) -> Result<Ty, Reject> {
        self.condition(cond)?;
        let before = self.assigned.clone();
        let then_ty = self.stmts(then)?;
        let after_then = std::mem::replace(&mut self.assigned, before.clone());
        let else_ty = match else_ {
            Some(stmts) => self.stmts(stmts)?,
            None => Ty::Unit,
        };
        let after_else = std::mem::replace(&mut self.assigned, before);
        // a branch that returns constrains nothing; otherwise only what both branches assign is certain
        self.assigned = match (then_ty, else_ty) {
            (Ty::Never, Ty::Never) => after_then,
            (Ty::Never, _) => after_else,
            (_, Ty::Never) => after_then,
            _ => after_then.intersection(&after_else).cloned().collect(),
        };
        Ok(match (then_ty, else_ty, else_.is_some()) {
            (Ty::Scalar(a), Ty::Scalar(b), true) if a == b => Ty::Scalar(a),
            (Ty::Scalar(a), Ty::Never, true) | (Ty::Never, Ty::Scalar(a), true) => Ty::Scalar(a),
            (Ty::Never, Ty::Never, true) => Ty::Never,
            _ => Ty::Unit,
        })
    }

    fn condition(&mut self, cond: &Expr) -> Result<(), Reject> {
        match self.value(cond)? {
            ScalarTy::Bool => Ok(()),
            // any number is truthy in Grenat: keep such code interpreted rather than guess
            t => Err(format!("uses a `{t}` as a condition")),
        }
    }

    fn method(&mut self, recv: &Expr, name: &str, args: &[grenat_ast::Arg]) -> Result<Ty, Reject> {
        use ScalarTy::*;
        if let ExprKind::Const(path) = &recv.kind
            && path.len() == 1
            && path[0].name == "Math"
            && name == "sqrt"
        {
            let [grenat_ast::Arg::Pos(arg)] = args else { return Err("calls `Math.sqrt` with bad arguments".into()) };
            if self.value(arg)? == Bool {
                return Err("takes the square root of a `Bool`".into());
            }
            self.typed.types.insert(recv as *const Expr, Ty::Unit);
            return Ok(Ty::Scalar(Float));
        }
        if !args.is_empty() {
            return Err(format!("calls method `{name}` with arguments"));
        }
        let t = self.value(recv)?;
        Ok(Ty::Scalar(match (t, name) {
            (Int, "to_i" | "abs") => Int,
            (Int, "to_f") => Float,
            (Int, "zero?" | "even?" | "odd?") => Bool,
            (Float, "to_i") => Int,
            (Float, "to_f" | "abs") => Float,
            (Float, "zero?") => Bool,
            _ => return Err(format!("calls `{t}.{name}`")),
        }))
    }

    fn read(&self, name: &str) -> Result<ScalarTy, Reject> {
        match self.vars.get(name) {
            Some(t) if self.assigned.contains(name) => Ok(*t),
            Some(_) => Err(format!("may read `{name}` before assigning it")),
            None if self.sigs.contains_key(name) => Err(format!("calls `{name}` without parentheses")),
            None => Err(format!("reads `{name}`, which is not a local variable")),
        }
    }

    fn define(&mut self, name: &str, t: ScalarTy) -> Result<(), Reject> {
        match self.vars.get(name) {
            Some(existing) if *existing != t => {
                return Err(format!("changes the type of `{name}` from `{existing}` to `{t}`"));
            }
            Some(_) => {}
            None => {
                self.vars.insert(name.to_string(), t);
                self.typed.locals.push((name.to_string(), t));
            }
        }
        self.assigned.insert(name.to_string());
        Ok(())
    }
}

/// Result type of `l op r`, exactly as the interpreter computes it.
fn binary(op: BinOp, l: ScalarTy, r: ScalarTy) -> Result<ScalarTy, Reject> {
    use BinOp::*;
    use ScalarTy::*;
    let numeric = |t: ScalarTy| t != Bool;
    Ok(match op {
        Add | Sub | Mul | Div | Rem if l == Int && r == Int => Int,
        Rem if numeric(l) && numeric(r) => return Err("uses `%` on a `Float`".into()),
        Add | Sub | Mul | Div if numeric(l) && numeric(r) => Float,
        Lt | Le | Gt | Ge if numeric(l) && numeric(r) => Bool,
        Eq | NotEq if (numeric(l) && numeric(r)) || (l == Bool && r == Bool) => Bool,
        Cmp if numeric(l) && numeric(r) => Int,
        And | Or if l == Bool && r == Bool => Bool,
        BitAnd | BitOr | BitXor if l == Int && r == Int => Int,
        Pow => return Err("uses `**`".into()),
        Shl | Shr => return Err("uses a bit shift".into()),
        _ => return Err(format!("applies an operator to `{l}` and `{r}`")),
    })
}

fn describe(kind: &ExprKind) -> &'static str {
    match kind {
        ExprKind::Str(_) => "strings",
        ExprKind::Symbol(_) => "symbols",
        ExprKind::Nil => "`nil`",
        ExprKind::Array(_) | ExprKind::Index { .. } => "arrays",
        ExprKind::Hash(_) => "hashes",
        ExprKind::Range { .. } => "ranges",
        ExprKind::IVar(_) | ExprKind::SelfRef => "object state",
        ExprKind::Const(_) => "constants",
        ExprKind::Call { block: Some(_), .. } => "blocks",
        ExprKind::Call { .. } => "a call that is not compiled",
        ExprKind::Case { .. } => "`case`",
        ExprKind::Begin(_) => "`begin`",
        ExprKind::Try(_) => "`?`",
        ExprKind::MultiAssign { .. } => "multiple assignment",
        ExprKind::Break(_) | ExprKind::Next(_) => "`break`/`next`",
        _ => "an unsupported construct",
    }
}
