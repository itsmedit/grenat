//! Translation of a typed function body into Cranelift IR.
//!
//! Semantics match the interpreter exactly (see each submodule). Errors are
//! not unwound: they release what the function holds and jump to the single
//! exit block, which returns a zero value and a status (see [`abi`](crate::abi)).
//! The same exit, with the `DEOPT` status, is taken for results native code
//! cannot represent (`nil`, an index out of range…): the interpreter then
//! runs the call again from the start.

mod arrays;
mod calls;
mod io;
mod numbers;
mod ownership;
mod strings;
mod structs;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{Block, BlockArg, FuncRef, GlobalValue, InstBuilder, MemFlagsData, Value, types};
use cranelift_frontend::{FunctionBuilder, Variable};
use grenat_ast::{BinOp, Expr, ExprKind, FnDef, Span};

use crate::abi::{LIMIT_OFFSET, POLL_OFFSET, SITE_OFFSET, Trap};
use crate::infer::{Flow, Signature, Typed};
use crate::liveness::{Edge, Plan};
use crate::runtime::{Rt, RuntimeRefs};
use crate::site::Site;
use crate::structs::Structs as StructTable;
use crate::ty::Ty;

pub(crate) use io::descriptors;
pub(crate) use ownership::Held;
use ownership::Reuse;

/// Compiled functions other functions may call from native code, by name.
pub(crate) type Callees<'a> = HashMap<&'a str, FuncRef>;

/// Everything a function's translation refers to besides its own body.
pub(crate) struct Env<'a> {
    pub callees: &'a Callees<'a>,
    pub runtime: &'a RuntimeRefs,
    pub structs: &'a StructTable,
    /// The shapes, in shape order (see [`shapes`](crate::shapes)).
    pub shapes: &'a [GlobalValue],
    /// The bytes of each string literal of the function.
    pub literals: &'a HashMap<String, GlobalValue>,
    /// Sites of the whole program, numbered in order.
    pub sites: &'a RefCell<Vec<Site>>,
}

pub(crate) struct Translator<'a, 'b> {
    b: FunctionBuilder<'b>,
    /// The function (for its sites).
    def: &'a FnDef,
    /// Span of the expression being translated (for its sites).
    span: Span,
    typed: &'a Typed,
    plan: &'a Plan,
    env: &'a Env<'a>,
    vars: HashMap<String, (Variable, Ty)>,
    /// Recursion depth of this call, passed in a register.
    depth: Value,
    /// The [`Context`](crate::abi) of the native call: recursion limit and
    /// checkpoint, read from memory (they never change during the call).
    ctx: Value,
    exit: Block,
    /// `None`: a procedure.
    ret: Option<Ty>,
    /// Variables owning a reference at the current point (see [`liveness`](crate::liveness)).
    owned: BTreeSet<String>,
    /// Owned temporaries not yet consumed, released if an error interrupts.
    pending: Vec<(Value, Ty)>,
    /// Struct constructions under way, which may reuse a dying record.
    reuse: Vec<Reuse>,
    /// Loops enclosing the current point.
    loops: usize,
}

impl<'a, 'b> Translator<'a, 'b> {
    /// Emits the whole function: entry (depth check), body, exit block.
    /// Parameters: the values, then `depth` and the context; results: the value, then the status.
    pub fn function(
        mut b: FunctionBuilder<'b>,
        def: &'a FnDef,
        sig: &Signature,
        typed: &'a Typed,
        plan: &'a Plan,
        env: &'a Env<'a>,
    ) -> FunctionBuilder<'b> {
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let params = b.block_params(entry).to_vec();
        let n = sig.params.len();
        let (depth, ctx) = (params[n], params[n + 1]);
        let exit = b.create_block();
        b.append_block_param(exit, crate::emit::ret_type(sig));
        b.append_block_param(exit, types::I64);

        let mut t = Translator {
            b,
            def,
            span: def.span,
            typed,
            plan,
            env,
            vars: HashMap::new(),
            depth,
            ctx,
            exit,
            ret: sig.ret,
            owned: BTreeSet::new(),
            pending: Vec::new(),
            reuse: Vec::new(),
            loops: 0,
        };
        for ((param, ty), value) in def.params.iter().zip(&sig.params).zip(&params) {
            let var = t.b.declare_var(ty.clif());
            t.b.def_var(var, *value);
            t.vars.insert(param.name.name.clone(), (var, *ty));
            if ty.is_heap() {
                t.owned.insert(param.name.name.clone());
            }
        }
        // every local gets a value up front; definite assignment guarantees it is never read
        for (name, ty) in &typed.locals {
            let var = t.b.declare_var(ty.clif());
            let zero = t.zero(*ty);
            t.b.def_var(var, zero);
            t.vars.insert(name.clone(), (var, *ty));
        }

        let limit = t.load(types::I64, ctx, LIMIT_OFFSET);
        let too_deep = t.b.ins().icmp(IntCC::SignedGreaterThan, depth, limit);
        t.trap_if(too_deep, Trap::StackOverflow as i64);
        t.checkpoint();
        for name in &plan.entry {
            t.drop_var(name);
        }

        match t.stmts(&def.body.stmts) {
            Some(held) => {
                let value = t.result(held);
                t.check_balanced();
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

    /// The value returned for `held`: a procedure drops it.
    fn result(&mut self, held: Held) -> Value {
        if self.ret.is_some() {
            return self.consume(held);
        }
        self.release(held);
        self.b.ins().iconst(types::I64, 0)
    }

    fn zero(&mut self, ty: Ty) -> Value {
        match ty {
            Ty::Float => self.b.ins().f64const(0.0),
            other => self.b.ins().iconst(other.clif(), 0),
        }
    }

    /// Leaves with a zero value and `status`.
    fn jump_exit_zero(&mut self, status: Value) {
        let zero = match self.ret {
            Some(ty) => self.zero(ty),
            None => self.b.ins().iconst(types::I64, 0),
        };
        self.b.ins().jump(self.exit, &[BlockArg::Value(zero), BlockArg::Value(status)]);
    }

    /// Continues in a block no one jumps to (code after `return` or a trap).
    fn unreachable_block(&mut self) {
        let dead = self.b.create_block();
        self.b.switch_to_block(dead);
    }

    /// If `cond` holds: release everything held and leave with `status`
    /// (a [`Trap`] code).
    fn trap_if(&mut self, cond: Value, status: i64) {
        self.stop_if(cond, status, None);
    }

    /// If `cond` holds: record the site, release everything held, and leave
    /// with `status`.
    fn stop_if(&mut self, cond: Value, status: i64, reason: Option<&'static str>) {
        let fail = self.b.create_block();
        let cont = self.b.create_block();
        self.b.ins().brif(cond, fail, &[], cont, &[]);
        self.b.switch_to_block(fail);
        let site = {
            let mut sites = self.env.sites.borrow_mut();
            sites.push(Site {
                function: self.def.name.name.clone(),
                function_span: self.def.span,
                span: self.span,
                reason,
            });
            sites.len() as i64 - 1
        };
        let site = self.b.ins().iconst(types::I64, site);
        self.store(site, self.ctx, SITE_OFFSET);
        let code = self.b.ins().iconst(types::I64, status);
        self.fail(code);
        self.b.switch_to_block(cont);
    }

    /// On every function entry and loop iteration: if the host requested a
    /// checkpoint (see `grenat_runtime::Poll`), asks it whether the task was
    /// cancelled — a native loop is otherwise impossible to stop. A read of a
    /// flag that rarely changes: no dependency between iterations.
    pub(super) fn checkpoint(&mut self) {
        let requested = self.b.ins().load(types::I8, MemFlagsData::new(), self.ctx, POLL_OFFSET);
        let poll = self.b.ins().iadd_imm_s(self.ctx, i64::from(POLL_OFFSET));
        let ask = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(requested, ask, &[], done, &[]);
        self.b.switch_to_block(ask);
        let stop = self.runtime_bool(Rt::Poll, &[poll]);
        self.trap_if(stop, Trap::Cancelled as i64);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// After a native call: leave at once with the callee's status if it failed.
    fn propagate_trap(&mut self, status: Value) {
        let failed = self.b.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
        let fail = self.b.create_block();
        let cont = self.b.create_block();
        self.b.ins().brif(failed, fail, &[], cont, &[]);
        self.b.switch_to_block(fail);
        self.fail(status);
        self.b.switch_to_block(cont);
    }

    fn bool_const(&mut self, b: bool) -> Value {
        self.b.ins().iconst(types::I8, i64::from(b))
    }

    fn load(&mut self, ty: cranelift_codegen::ir::Type, addr: Value, offset: i32) -> Value {
        self.b.ins().load(ty, MemFlagsData::trusted(), addr, offset)
    }

    fn store(&mut self, value: Value, addr: Value, offset: i32) {
        self.b.ins().store(MemFlagsData::trusted(), value, addr, offset);
    }

    /// Calls a runtime function, returning its results.
    fn runtime(&mut self, rt: Rt, args: &[Value]) -> Vec<Value> {
        let call = self.b.ins().call(self.env.runtime[&rt], args);
        self.b.inst_results(call).to_vec()
    }

    /// A runtime predicate (`i64` 0 or 1) as a `Bool`.
    fn runtime_bool(&mut self, rt: Rt, args: &[Value]) -> Value {
        let r = self.runtime(rt, args)[0];
        self.b.ins().icmp_imm_s(IntCC::NotEqual, r, 0)
    }

    /// Address of the shape of `ty` (data of the module).
    fn shape(&mut self, ty: Ty) -> Value {
        let index = crate::shapes::index(ty, self.env.structs.count());
        self.b.ins().symbol_value(types::I64, self.env.shapes[index])
    }

    // ── Statements ───────────────────────────────────────────

    /// Value of the last statement, or `None` for a statement without value;
    /// the values of the other statements are released.
    fn stmts(&mut self, stmts: &[Expr]) -> Option<Held> {
        let (last, init) = stmts.split_last()?;
        for stmt in init {
            if let Some(held) = self.eval(stmt) {
                self.release(held);
            }
        }
        self.eval(last)
    }

    /// Ends with `return` (or a branch of which all do).
    fn never(&self, stmts: Option<&[Expr]>) -> bool {
        stmts.and_then(<[Expr]>::last).is_some_and(|s| self.typed.flow(s) == Flow::Never)
    }

    /// Evaluates `e`. A heap value is owned by the caller (and pending until
    /// consumed or released), except a read of a variable that is not its
    /// last use, which is borrowed from the variable.
    fn eval(&mut self, e: &Expr) -> Option<Held> {
        let outer = std::mem::replace(&mut self.span, e.span);
        let held = self.eval_kind(e);
        self.span = outer;
        let held = held?;
        debug_assert!(held.owned || !held.ty.is_heap() || matches!(e.kind, ExprKind::Var(_)), "borrowed: {e:?}");
        Some(self.hold(held))
    }

    /// A scalar value.
    fn scalar(&mut self, e: &Expr) -> Value {
        self.eval(e).expect("value expected (checked by infer)").value
    }

    fn eval_kind(&mut self, e: &Expr) -> Option<Held> {
        let scalar = |value| Some(Held::scalar(value, self.typed.ty(e)));
        match &e.kind {
            ExprKind::Int(n) => scalar(self.b.ins().iconst(types::I64, *n)),
            ExprKind::Float(f) => scalar(self.b.ins().f64const(*f)),
            ExprKind::Bool(b) => scalar(self.bool_const(*b)),
            ExprKind::Str(segs) => Some(self.string(segs)),
            ExprKind::Array(items) => Some(self.array_literal(e, items)),
            ExprKind::Var(_) if self.typed.methods.contains_key(&(e as *const Expr)) => self.call(e, None, &[], None),
            ExprKind::Var(name) => Some(self.read(e, name)),
            ExprKind::Assign { target, value } => self.assign(e, target, value),
            ExprKind::OpAssign { op, target, value } => self.op_assign(e, *op, target, value),
            ExprKind::Binary { op: BinOp::And, lhs, rhs } => scalar(self.short_circuit(e, lhs, rhs, true)),
            ExprKind::Binary { op: BinOp::Or, lhs, rhs } => scalar(self.short_circuit(e, lhs, rhs, false)),
            ExprKind::Binary { op, lhs, rhs } => self.binary(e, *op, lhs, rhs),
            ExprKind::Unary { op, expr } => {
                let v = self.scalar(expr);
                scalar(self.unary(*op, v, self.typed.ty(expr)))
            }
            ExprKind::If { cond, then, else_ } => self.if_expr(e, cond, then, else_.as_deref()),
            ExprKind::While { cond, body } => {
                self.while_loop(e, cond, body);
                None
            }
            ExprKind::Return(value) => {
                self.return_(value.as_deref());
                None
            }
            ExprKind::Index { recv, args } => Some(self.index(e, recv, &args[0])),
            ExprKind::Call { recv, args, block, .. } => self.call(e, recv.as_deref(), args, block.as_deref()),
            other => unreachable!("rejected by infer: {other:?}"),
        }
    }

    // ── Variables ────────────────────────────────────────────

    /// Reads a variable: its last use moves the reference out of it.
    fn read(&mut self, e: &Expr, name: &str) -> Held {
        let (var, ty) = self.vars[name];
        let value = self.b.use_var(var);
        let moved = ty.is_heap() && self.plan.moves(e);
        if moved {
            let owned = self.owned.remove(name);
            debug_assert!(owned, "`{name}` moved without owning");
        }
        Held { value, ty, owned: moved }
    }

    /// Gives `name` the value of `held`; `assign` is the assignment, if any
    /// (`None` for a block parameter).
    fn define(&mut self, name: &str, held: Held, assign: Option<&Expr>) {
        let (var, ty) = self.vars[name];
        let value = self.consume(held);
        self.b.def_var(var, value);
        if ty.is_heap() {
            let fresh = self.owned.insert(name.to_string());
            debug_assert!(fresh, "`{name}` overwritten while owning a reference");
            if assign.is_some_and(|a| self.plan.dead_after(a)) {
                self.drop_var(name);
            }
        }
    }

    fn assign(&mut self, e: &Expr, target: &Expr, value: &Expr) -> Option<Held> {
        match &target.kind {
            ExprKind::Var(name) => {
                let held = self.eval(value).expect("a value");
                let ty = held.ty;
                self.define(name, held, Some(e));
                // an assignment to an object has no value (see infer)
                (!ty.is_heap()).then(|| {
                    let (var, _) = self.vars[name];
                    Held::scalar(self.b.use_var(var), ty)
                })
            }
            ExprKind::Index { recv, args } => {
                let item = self.operand(value, &[recv, &args[0]]);
                let array = self.operand(recv, &[&args[0]]);
                let index = self.scalar(&args[0]);
                self.store_element(array, index, item);
                self.release(array);
                None
            }
            _ => unreachable!("rejected by infer"),
        }
    }

    fn op_assign(&mut self, e: &Expr, op: BinOp, target: &Expr, value: &Expr) -> Option<Held> {
        match &target.kind {
            ExprKind::Var(name) => {
                let current = self.operand(target, &[value]);
                let rhs = self.operand(value, &[]);
                let result = self.binary_held(op, current, rhs);
                let result = self.hold(result);
                let ty = result.ty;
                self.define(name, result, Some(e));
                (!ty.is_heap()).then(|| {
                    let (var, _) = self.vars[name];
                    Held::scalar(self.b.use_var(var), ty)
                })
            }
            ExprKind::Index { recv, args } => {
                let array = self.operand(recv, &[&args[0], value]);
                let index = self.scalar(&args[0]);
                let current = self.element(array, index, "an index out of range (`nil`)");
                let current = self.hold(current);
                let rhs = self.operand(value, &[]);
                let result = self.binary_held(op, current, rhs);
                let result = self.hold(result);
                self.store_element(array, index, result);
                self.release(array);
                None
            }
            _ => unreachable!("rejected by infer"),
        }
    }

    // ── Control flow ─────────────────────────────────────────

    /// Drops the variables that die on `edge` of `e`.
    fn edge(&mut self, e: &Expr, edge: Edge) {
        for name in self.plan.drops(e, edge).to_vec() {
            self.drop_var(&name);
        }
    }

    fn if_expr(&mut self, e: &Expr, cond: &Expr, then: &[Expr], else_: Option<&[Expr]>) -> Option<Held> {
        let result = match self.typed.flow(e) {
            Flow::Value(t) => Some(t),
            _ => None,
        };
        let c = self.scalar(cond);
        let then_block = self.b.create_block();
        let else_block = self.b.create_block();
        let merge = self.b.create_block();
        if let Some(t) = result {
            self.b.append_block_param(merge, t.clif());
        }
        self.b.ins().brif(c, then_block, &[], else_block, &[]);
        let (owned, pending) = (self.owned.clone(), self.pending.clone());
        let mut after = None;
        for (block, stmts, edge) in [(then_block, Some(then), Edge::Then), (else_block, else_, Edge::Else)] {
            self.b.switch_to_block(block);
            (self.owned, self.pending) = (owned.clone(), pending.clone());
            self.edge(e, edge);
            let value = stmts.and_then(|s| self.stmts(s));
            let never = self.never(stmts);
            match result {
                Some(t) => {
                    // a branch that returned is unreachable here: any value of the right type will do
                    let v = match value {
                        Some(held) if !never => self.consume(held),
                        _ => self.zero(t),
                    };
                    self.b.ins().jump(merge, &[BlockArg::Value(v)]);
                }
                None => {
                    if let Some(held) = value.filter(|_| !never) {
                        self.release(held);
                    }
                    self.b.ins().jump(merge, &[]);
                }
            }
            if !never {
                debug_assert!(after.as_ref().is_none_or(|a| *a == self.owned), "branches disagree on ownership");
                debug_assert_eq!(self.pending, pending, "a branch leaves a temporary");
                after = Some(self.owned.clone());
            }
        }
        self.b.switch_to_block(merge);
        (self.owned, self.pending) = (after.unwrap_or(owned), pending);
        result.map(|ty| Held { value: self.b.block_params(merge)[0], ty, owned: ty.is_heap() })
    }

    fn while_loop(&mut self, e: &Expr, cond: &Expr, body: &[Expr]) {
        let header = self.b.create_block();
        let body_block = self.b.create_block();
        let after = self.b.create_block();
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(header);
        let at_header = self.owned.clone();
        let c = self.scalar(cond);
        let decided = self.owned.clone();
        self.b.ins().brif(c, body_block, &[], after, &[]);

        self.b.switch_to_block(body_block);
        self.edge(e, Edge::Body);
        self.loops += 1;
        let value = self.stmts(body);
        self.loops -= 1;
        if !self.never(Some(body)) {
            if let Some(held) = value {
                self.release(held);
            }
            debug_assert_eq!(self.owned, at_header, "a loop body changes ownership");
            self.checkpoint();
            self.b.ins().jump(header, &[]);
        }

        self.b.switch_to_block(after);
        self.owned = decided;
        self.edge(e, Edge::Exit);
    }

    fn return_(&mut self, value: Option<&Expr>) {
        let v = match value.and_then(|v| self.eval(v)) {
            Some(held) => self.result(held),
            None => self.b.ins().iconst(types::I64, 0),
        };
        debug_assert!(self.owned.is_empty(), "still owned at `return`: {:?}", self.owned);
        // temporaries of enclosing expressions are abandoned
        self.release_all();
        let ok = self.b.ins().iconst(types::I64, 0);
        self.b.ins().jump(self.exit, &[BlockArg::Value(v), BlockArg::Value(ok)]);
        self.unreachable_block();
    }

    /// `a && b` / `a || b` on booleans: `b` only runs when it decides the result.
    fn short_circuit(&mut self, e: &Expr, lhs: &Expr, rhs: &Expr, and: bool) -> Value {
        let l = self.scalar(lhs);
        let rhs_block = self.b.create_block();
        let skip = self.b.create_block();
        let merge = self.b.create_block();
        self.b.append_block_param(merge, types::I8);
        if and {
            self.b.ins().brif(l, rhs_block, &[], skip, &[]);
        } else {
            self.b.ins().brif(l, skip, &[], rhs_block, &[]);
        }
        let decided = self.owned.clone();
        self.b.switch_to_block(rhs_block);
        self.edge(e, Edge::Rhs);
        let r = self.scalar(rhs);
        self.b.ins().jump(merge, &[BlockArg::Value(r)]);
        let after_rhs = std::mem::replace(&mut self.owned, decided);

        self.b.switch_to_block(skip);
        self.edge(e, Edge::Skip);
        self.b.ins().jump(merge, &[BlockArg::Value(l)]);
        debug_assert_eq!(self.owned, after_rhs, "`&&`/`||` operands disagree on ownership");

        self.b.switch_to_block(merge);
        self.b.block_params(merge)[0]
    }
}
