//! Typing of a function body for native compilation.
//!
//! Only a subset of Grenat is compiled; anything else is rejected with a
//! reason and the function stays interpreted. Beyond types, this pass enforces
//! *definite assignment*: the interpreter raises `NameError` when a local is
//! read before being assigned, whereas native code would read garbage, so
//! such functions must not be compiled.
//!
//! The element type of an empty literal `[]` comes from how the array is used
//! later (`xs = []` then `xs << 1`): a first pass learns the types of the
//! locals, a second pass types the body again knowing them.

mod calls;
mod methods;
mod ops;

use std::collections::{HashMap, HashSet};

use grenat_ast::{Arg, Block, Expr, ExprKind, FnDef};

use crate::structs::Structs;
use crate::ty::{Elem, StructId, Ty};

pub(crate) use methods::{Method, StrOp};

/// Native signature of a compilable function.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Signature {
    pub params: Vec<Ty>,
    pub ret: Ty,
}

pub(crate) type Signatures<'p> = HashMap<&'p str, Signature>;

/// Static type of an expression.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Flow {
    Value(Ty),
    /// No value (a loop, an `if` without `else` used as a statement, an assignment to an object).
    Unit,
    /// Control never continues past it (`return`).
    Never,
}

/// A struct built natively: its type and, for each argument in source
/// order, the field it initializes.
#[derive(Debug, Clone)]
pub(crate) struct Construct {
    pub id: StructId,
    pub fields: Vec<usize>,
}

/// A block inlined as a loop body (`n.times do |i|`, `xs.each do |x|`).
#[derive(Debug, Clone)]
pub(crate) struct Inlined {
    /// The block's parameters, as locals of the function.
    pub params: Vec<String>,
}

/// Result of typing a function body, consumed by liveness and translation.
#[derive(Debug, Default)]
pub(crate) struct Typed {
    /// Every local, parameters included.
    pub vars: HashMap<String, Ty>,
    /// Locals other than parameters, in order of first assignment.
    pub locals: Vec<(String, Ty)>,
    /// Type of every expression, keyed by address in the AST.
    pub types: HashMap<*const Expr, Flow>,
    /// Resolved method calls (and indexing, field reads).
    pub methods: HashMap<*const Expr, Method>,
    /// Struct constructions.
    pub constructs: HashMap<*const Expr, Construct>,
    /// Inlined blocks, keyed by the call receiving them.
    pub blocks: HashMap<*const Expr, Inlined>,
    /// Modifies an array (its own code, not counting callees).
    pub mutates: bool,
    /// Compiled functions it calls.
    pub calls: HashSet<String>,
}

impl Typed {
    pub fn flow(&self, e: &Expr) -> Flow {
        self.types[&(e as *const Expr)]
    }

    /// Type of an expression that has a value.
    pub fn ty(&self, e: &Expr) -> Ty {
        match self.flow(e) {
            Flow::Value(t) => t,
            other => unreachable!("value expected, typed {other:?}"),
        }
    }

    pub fn method(&self, e: &Expr) -> Method {
        self.methods[&(e as *const Expr)]
    }

    pub fn is_heap_var(&self, name: &str) -> bool {
        self.vars.get(name).is_some_and(|t| t.is_heap())
    }
}

pub(crate) type Reject = String;

/// Types `def` against the signatures of every compilable function.
pub(crate) fn infer(def: &FnDef, sig: &Signature, sigs: &Signatures, structs: &Structs) -> Result<Typed, Reject> {
    if !def.body.rescues.is_empty() || def.body.ensure.is_some() {
        return Err("uses `rescue`/`ensure`".into());
    }
    let first = Infer::run(def, sig, sigs, structs, HashMap::new())?;
    if !first.types.values().any(|f| matches!(f, Flow::Value(t) if t.is_unknown())) {
        return Ok(first);
    }
    let second = Infer::run(def, sig, sigs, structs, first.vars)?;
    let unknown = second.types.values().any(|f| matches!(f, Flow::Value(t) if t.is_unknown()))
        || second.vars.values().any(|t| t.is_unknown());
    if unknown {
        return Err("uses an empty array `[]` whose element type is unknown".into());
    }
    Ok(second)
}

pub(crate) struct Infer<'a, 'p> {
    sigs: &'a Signatures<'p>,
    structs: &'a Structs,
    ret: Ty,
    /// Types learned by the first pass.
    seed: HashMap<String, Ty>,
    /// Locals certainly assigned at this point of the body.
    assigned: HashSet<String>,
    /// Parameters and locals assigned outside block parameters.
    outer: HashSet<String>,
    typed: Typed,
}

impl<'a, 'p> Infer<'a, 'p> {
    fn run(
        def: &FnDef,
        sig: &Signature,
        sigs: &'a Signatures<'p>,
        structs: &'a Structs,
        seed: HashMap<String, Ty>,
    ) -> Result<Typed, Reject> {
        let mut cx = Infer {
            sigs,
            structs,
            ret: sig.ret,
            seed,
            assigned: HashSet::new(),
            outer: HashSet::new(),
            typed: Typed::default(),
        };
        for (param, ty) in def.params.iter().zip(&sig.params) {
            cx.typed.vars.insert(param.name.name.clone(), *ty);
            cx.assigned.insert(param.name.name.clone());
            cx.outer.insert(param.name.name.clone());
        }
        let body = cx.stmts(&def.body.stmts, Some(sig.ret))?;
        match body {
            Flow::Value(t) => cx.expect(def.body.stmts.last().expect("a value"), t, sig.ret, "returns")?,
            Flow::Never => {}
            Flow::Unit => return Err("its last statement has no value".into()),
        }
        Ok(cx.typed)
    }

    fn show(&self, ty: Ty) -> String {
        self.structs.show(ty)
    }

    // ── Statements and control flow ──────────────────────────

    /// `expected`: type wanted for the last statement, if any.
    fn stmts(&mut self, stmts: &[Expr], expected: Option<Ty>) -> Result<Flow, Reject> {
        let mut last = Flow::Unit;
        for (i, stmt) in stmts.iter().enumerate() {
            if last == Flow::Never {
                return Err("has unreachable code after `return`".into());
            }
            last = self.expr(stmt, if i + 1 == stmts.len() { expected } else { None })?;
        }
        Ok(last)
    }

    /// Types an expression that must have a value.
    fn value(&mut self, e: &Expr, expected: Option<Ty>) -> Result<Ty, Reject> {
        match self.expr(e, expected)? {
            Flow::Value(t) => Ok(t),
            _ => Err("uses a statement where a value is expected".into()),
        }
    }

    fn expr(&mut self, e: &Expr, expected: Option<Ty>) -> Result<Flow, Reject> {
        let flow = self.expr_kind(e, expected)?;
        self.typed.types.insert(e as *const Expr, flow);
        Ok(flow)
    }

    fn expr_kind(&mut self, e: &Expr, expected: Option<Ty>) -> Result<Flow, Reject> {
        Ok(match &e.kind {
            ExprKind::Int(_) => Flow::Value(Ty::Int),
            ExprKind::Float(_) => Flow::Value(Ty::Float),
            ExprKind::Bool(_) => Flow::Value(Ty::Bool),
            ExprKind::Str(segs) => Flow::Value(self.string(segs)?),
            ExprKind::Array(items) => Flow::Value(self.array(items, expected)?),
            ExprKind::Var(name) => Flow::Value(self.read(name)?),
            ExprKind::Assign { target, value } => self.assign(target, value)?,
            ExprKind::OpAssign { op, target, value } => self.op_assign(*op, target, value)?,
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs)?,
            ExprKind::Unary { op, expr } => Flow::Value(self.unary(*op, expr)?),
            ExprKind::If { cond, then, else_ } => self.if_expr(cond, then, else_.as_deref(), expected)?,
            ExprKind::While { cond, body } => {
                self.condition(cond)?;
                self.loop_body(|cx| cx.stmts(body, None).map(|_| ()))?;
                Flow::Unit
            }
            ExprKind::Return(value) => {
                let Some(value) = value else { return Err("returns nothing".into()) };
                let t = self.value(value, Some(self.ret))?;
                self.expect(value, t, self.ret, "returns")?;
                Flow::Never
            }
            ExprKind::Index { recv, args } => Flow::Value(self.index(e, recv, args)?),
            ExprKind::Call { recv, name, args, block, safe: false, .. } => {
                self.call(e, recv.as_deref(), &name.name, args, block.as_deref())?
            }
            other => return Err(format!("uses {}", describe(other))),
        })
    }

    fn if_expr(&mut self, cond: &Expr, then: &[Expr], else_: Option<&[Expr]>, expected: Option<Ty>) -> Result<Flow, Reject> {
        self.condition(cond)?;
        let before = self.assigned.clone();
        let then_flow = self.stmts(then, expected)?;
        let after_then = std::mem::replace(&mut self.assigned, before.clone());
        let else_flow = match else_ {
            Some(stmts) => self.stmts(stmts, expected)?,
            None => Flow::Unit,
        };
        let after_else = std::mem::replace(&mut self.assigned, before);
        // a branch that returns constrains nothing; otherwise only what both branches assign is certain
        self.assigned = match (then_flow, else_flow) {
            (Flow::Never, Flow::Never) => after_then,
            (Flow::Never, _) => after_else,
            (_, Flow::Never) => after_then,
            _ => after_then.intersection(&after_else).cloned().collect(),
        };
        Ok(match (then_flow, else_flow, else_.is_some()) {
            (Flow::Value(a), Flow::Value(b), true) if a == b => Flow::Value(a),
            // `if c then [] else [1] end`
            (Flow::Value(a), Flow::Value(b), true) if a.is_unknown() && matches!(b, Ty::Array(_)) => Flow::Value(b),
            (Flow::Value(a), Flow::Value(b), true) if b.is_unknown() && matches!(a, Ty::Array(_)) => Flow::Value(a),
            (Flow::Value(a), Flow::Never, true) | (Flow::Never, Flow::Value(a), true) => Flow::Value(a),
            (Flow::Never, Flow::Never, true) => Flow::Never,
            _ => Flow::Unit,
        })
    }

    /// A loop body: it may run zero times, so nothing it assigns is certain afterwards.
    fn loop_body(&mut self, body: impl FnOnce(&mut Self) -> Result<(), Reject>) -> Result<(), Reject> {
        let before = self.assigned.clone();
        body(self)?;
        self.assigned = before;
        Ok(())
    }

    fn condition(&mut self, cond: &Expr) -> Result<(), Reject> {
        match self.value(cond, None)? {
            Ty::Bool => Ok(()),
            // any value but `false`/`nil` is truthy in Grenat: keep such code interpreted rather than guess
            t => Err(format!("uses `{}` as a condition", self.show(t))),
        }
    }

    /// An inlined block: its parameters become locals, which must not clash
    /// with the function's (a block parameter shadows an outer variable).
    fn block(&mut self, call: &Expr, block: &Block, params: &[Ty]) -> Result<(), Reject> {
        if !block.body.rescues.is_empty() || block.body.ensure.is_some() {
            return Err("uses `rescue`/`ensure` in a block".into());
        }
        if block.params.len() != params.len() {
            return Err(format!("gives a block {} parameter(s) instead of {}", block.params.len(), params.len()));
        }
        let names: Vec<String> = block.params.iter().map(|p| p.name.name.clone()).collect();
        for (name, ty) in names.iter().zip(params) {
            // natively, the parameter and the variable would be one and the same
            if self.outer.contains(name) {
                return Err(format!("names a block parameter `{name}` like a variable of the function"));
            }
            self.bind(name, *ty)?;
        }
        self.loop_body(|cx| {
            if cx.stmts(&block.body.stmts, None)? == Flow::Never {
                return Err("returns from inside a block".into());
            }
            Ok(())
        })?;
        for name in &names {
            self.assigned.remove(name);
        }
        self.typed.blocks.insert(call as *const Expr, Inlined { params: names });
        Ok(())
    }

    // ── Locals ───────────────────────────────────────────────

    fn read(&self, name: &str) -> Result<Ty, Reject> {
        match self.typed.vars.get(name) {
            Some(t) if self.assigned.contains(name) => Ok(*t),
            Some(_) => Err(format!("may read `{name}` before assigning it")),
            None if self.sigs.contains_key(name) => Err(format!("calls `{name}` without parentheses")),
            None => Err(format!("reads `{name}`, which is not a local variable")),
        }
    }

    /// Type a value assigned to `name` should have (`[]` takes it).
    fn declared(&self, name: &str) -> Option<Ty> {
        self.typed.vars.get(name).or_else(|| self.seed.get(name)).copied()
    }

    /// Assignment to a local.
    fn define(&mut self, name: &str, t: Ty) -> Result<(), Reject> {
        self.outer.insert(name.to_string());
        self.bind(name, t)
    }

    /// Gives `name` a value of type `t` (assignment or block parameter).
    fn bind(&mut self, name: &str, t: Ty) -> Result<(), Reject> {
        match self.typed.vars.get(name).copied() {
            Some(existing) if existing == t || t.is_unknown() && matches!(existing, Ty::Array(_)) => {}
            Some(existing) if existing.is_unknown() && matches!(t, Ty::Array(_)) => {
                self.typed.vars.insert(name.to_string(), t);
                if let Some(local) = self.typed.locals.iter_mut().find(|(n, _)| n == name) {
                    local.1 = t;
                }
            }
            Some(existing) => {
                return Err(format!("changes the type of `{name}` from `{}` to `{}`", self.show(existing), self.show(t)));
            }
            None => {
                self.typed.vars.insert(name.to_string(), t);
                self.typed.locals.push((name.to_string(), t));
            }
        }
        self.assigned.insert(name.to_string());
        Ok(())
    }

    /// Checks that `e`, typed `actual`, can be used where `expected` is
    /// required. An `[]` held by a local learns its element type here.
    fn expect(&mut self, e: &Expr, actual: Ty, expected: Ty, what: &str) -> Result<(), Reject> {
        if actual == expected {
            return Ok(());
        }
        if actual.is_unknown() && matches!(expected, Ty::Array(_)) {
            if let ExprKind::Var(name) = &e.kind {
                self.define(name, expected)?;
            }
            return Ok(());
        }
        Err(format!("{what} `{}` instead of `{}`", self.show(actual), self.show(expected)))
    }

    /// Element type of the array `recv` when an element typed `item` is stored into it.
    fn store_elem(&mut self, recv: &Expr, array: Elem, item: Ty) -> Result<(), Reject> {
        let Some(elem) = item.elem() else { return Err("puts an array inside an array".into()) };
        match array {
            Elem::Unknown => self.expect(recv, Ty::Array(Elem::Unknown), Ty::Array(elem), "stores"),
            known if known == elem => Ok(()),
            known => Err(format!(
                "stores `{}` in an array of `{}`",
                self.show(item),
                self.structs.show_elem(known)
            )),
        }
    }
}

/// Positional arguments only.
fn positional<'e>(args: &'e [Arg], what: &str) -> Result<Vec<&'e Expr>, Reject> {
    args.iter()
        .map(|a| match a {
            Arg::Pos(e) => Ok(e),
            _ => Err(format!("calls `{what}` with named or block arguments")),
        })
        .collect()
}

fn describe(kind: &ExprKind) -> &'static str {
    match kind {
        ExprKind::Symbol(_) => "symbols",
        ExprKind::Nil => "`nil`",
        ExprKind::Hash(_) => "hashes",
        ExprKind::Range { .. } => "ranges",
        ExprKind::IVar(_) | ExprKind::SelfRef => "object state",
        ExprKind::Const(_) => "constants",
        ExprKind::Call { safe: true, .. } => "`&.`",
        ExprKind::Case { .. } => "`case`",
        ExprKind::Begin(_) => "`begin`",
        ExprKind::Try(_) => "`?`",
        ExprKind::MultiAssign { .. } => "multiple assignment",
        ExprKind::Break(_) | ExprKind::Next(_) => "`break`/`next`",
        _ => "an unsupported construct",
    }
}
