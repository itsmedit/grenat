//! Reference counting (Perceus): `dup`, `drop`, ownership of values being
//! computed, release on error paths, and drop-reuse of dying records.
//!
//! A heap value handled by the translator is either *owned* (the translator
//! must consume or release it: it is then "pending") or *borrowed* from a
//! variable that keeps owning it. Only reads of a variable are borrowed, and
//! [`Translator::operand`] makes sure the variable cannot lose its reference
//! while the borrowed value is still in use.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{InstBuilder, Value, types};
use cranelift_frontend::Variable;
use grenat_ast::{Expr, ExprKind};
use grenat_runtime::layout;

use super::Translator;
use crate::abi::DEOPT;
use crate::runtime::Rt;
use crate::ty::Ty;
use crate::walk::mentions;

/// A value being computed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Held {
    pub value: Value,
    pub ty: Ty,
    /// For a heap value: the translator holds a reference to release.
    pub owned: bool,
}

impl Held {
    pub fn scalar(value: Value, ty: Ty) -> Held {
        Held { value, ty, owned: false }
    }

    /// A newly created object (or one whose reference was handed over).
    pub fn object(value: Value, ty: Ty) -> Held {
        Held { value, ty, owned: true }
    }
}

/// A struct construction under way: a record dying while its fields are
/// evaluated gives its memory to the new one (Perceus' drop-reuse).
pub(crate) struct Reuse {
    fields: usize,
    /// The memory to reuse, or null (a variable: it may be set in one branch only).
    token: Variable,
    /// Shape of the record the token came from, once one did.
    source: Option<Ty>,
    /// Loop nesting of the construction: a record dying in a loop nested
    /// inside it could die several times, and only one token can be reused.
    loops: usize,
}

impl Translator<'_, '_> {
    // ── Counting ─────────────────────────────────────────────

    pub(super) fn dup(&mut self, obj: Value) {
        let rc = self.load(types::I64, obj, layout::RC);
        let rc = self.b.ins().iadd_imm_s(rc, 1);
        self.store(rc, obj, layout::RC);
    }

    /// Drops a reference; the last one frees the object (through the runtime,
    /// which releases its children).
    pub(super) fn drop_ref(&mut self, obj: Value, ty: Ty) {
        let rc = self.load(types::I64, obj, layout::RC);
        let last = self.b.ins().icmp_imm_s(IntCC::Equal, rc, 1);
        let free = self.b.create_block();
        let decrement = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(last, free, &[], decrement, &[]);
        self.b.switch_to_block(free);
        let shape = self.shape(ty);
        self.runtime(Rt::Free, &[obj, shape]);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(decrement);
        let rc = self.b.ins().iadd_imm_s(rc, -1);
        self.store(rc, obj, layout::RC);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    // ── Ownership of values being computed ───────────────────

    /// Registers an owned value as pending (see [`Translator::eval`]).
    pub(super) fn hold(&mut self, held: Held) -> Held {
        if held.owned && held.ty.is_heap() {
            self.pending.push((held.value, held.ty));
        }
        held
    }

    /// Forgets a pending value whose reference was handed over.
    pub(super) fn take(&mut self, value: Value) {
        let i = self.pending.iter().rposition(|(v, _)| *v == value).expect("pending value");
        self.pending.remove(i);
    }

    /// The reference of `held`, for a consumer that keeps it.
    pub(super) fn consume(&mut self, held: Held) -> Value {
        if held.ty.is_heap() {
            if held.owned {
                self.take(held.value);
            } else {
                self.dup(held.value);
            }
        }
        held.value
    }

    /// Done with `held`: its reference, if owned, is dropped.
    pub(super) fn release(&mut self, held: Held) {
        if held.ty.is_heap() && held.owned {
            self.take(held.value);
            if !self.reuse_record(held) {
                self.drop_ref(held.value, held.ty);
            }
        }
    }

    /// Evaluates an operand that stays in use while `later` operands are
    /// evaluated. A borrowed read of a variable those operands mention (and
    /// could move or reassign) takes its own reference.
    pub(super) fn operand(&mut self, e: &Expr, later: &[&Expr]) -> Held {
        let held = self.eval(e).expect("a value");
        if let ExprKind::Var(name) = &e.kind
            && held.ty.is_heap()
            && !held.owned
            && later.iter().any(|l| mentions(l, name))
        {
            self.dup(held.value);
            return self.hold(Held::object(held.value, held.ty));
        }
        held
    }

    pub(super) fn drop_var(&mut self, name: &str) {
        let owned = self.owned.remove(name);
        debug_assert!(owned, "`{name}` dropped without owning");
        let (var, ty) = self.vars[name];
        let value = self.b.use_var(var);
        self.drop_ref(value, ty);
    }

    /// At a normal exit: nothing may remain owned.
    pub(super) fn check_balanced(&self) {
        debug_assert!(self.owned.is_empty(), "still owned at exit: {:?}", self.owned);
        debug_assert!(self.pending.is_empty(), "still pending at exit: {:?}", self.pending);
        debug_assert!(self.reuse.is_empty());
    }

    // ── Leaving early ────────────────────────────────────────

    /// Releases every reference held at this point, without changing the
    /// translator's state (code after this point is still translated).
    pub(super) fn release_all(&mut self) {
        let owned: Vec<String> = self.owned.iter().cloned().collect();
        for name in owned {
            let (var, ty) = self.vars[&name];
            let value = self.b.use_var(var);
            self.drop_ref(value, ty);
        }
        for (value, ty) in self.pending.clone() {
            self.drop_ref(value, ty);
        }
        let tokens: Vec<(Variable, Ty)> = self.reuse.iter().filter_map(|r| r.source.map(|s| (r.token, s))).collect();
        for (token, source) in tokens {
            let token = self.b.use_var(token);
            let shape = self.shape(source);
            self.runtime(Rt::FreeToken, &[token, shape]);
        }
    }

    /// Error exit: release everything, leave with `status`.
    pub(super) fn fail(&mut self, status: Value) {
        self.release_all();
        self.jump_exit_zero(status);
    }

    /// Leaves with `DEOPT` if `cond` holds: the interpreter reruns the call.
    /// `reason`: what native code cannot represent, for a standalone executable.
    pub(super) fn deopt_if(&mut self, cond: Value, reason: &'static str) {
        self.stop_if(cond, DEOPT, Some(reason));
    }

    /// Deoptimizes when a runtime function returned null.
    pub(super) fn deopt_if_null(&mut self, obj: Value, reason: &'static str) {
        let null = self.b.ins().icmp_imm_s(IntCC::Equal, obj, 0);
        self.deopt_if(null, reason);
    }

    // ── Drop-reuse ───────────────────────────────────────────

    /// Opens a construction of a record of `fields` fields.
    pub(super) fn open_reuse(&mut self, fields: usize) {
        let token = self.b.declare_var(types::I64);
        let null = self.b.ins().iconst(types::I64, 0);
        self.b.def_var(token, null);
        self.reuse.push(Reuse { fields, token, source: None, loops: self.loops });
    }

    /// Closes the innermost construction: the memory to reuse, or null.
    pub(super) fn close_reuse(&mut self) -> Value {
        let reuse = self.reuse.pop().expect("an open construction");
        self.b.use_var(reuse.token)
    }

    /// Instead of dropping a record, hands its memory to the construction
    /// under way when it has the same size and no memory yet.
    fn reuse_record(&mut self, held: Held) -> bool {
        let Ty::Struct(id) = held.ty else { return false };
        let fields = self.env.structs.get(id).fields.len();
        let Some(reuse) = self.reuse.last() else { return false };
        if reuse.fields != fields || reuse.source.is_some() || reuse.loops != self.loops {
            return false;
        }
        let token = reuse.token;
        let def = self.env.structs.get(id);
        let memory = if def.fields.iter().any(|(_, t)| t.is_heap()) {
            let shape = self.shape(held.ty);
            self.runtime(Rt::DropReuse, &[held.value, shape])[0]
        } else {
            // no field to release: the memory is reusable exactly when this was the last reference
            let rc = self.load(types::I64, held.value, layout::RC);
            let last = self.b.ins().icmp_imm_s(IntCC::Equal, rc, 1);
            let decremented = self.b.ins().iadd_imm_s(rc, -1);
            let kept = self.b.ins().select(last, rc, decremented);
            self.store(kept, held.value, layout::RC);
            let null = self.b.ins().iconst(types::I64, 0);
            self.b.ins().select(last, held.value, null)
        };
        self.b.def_var(token, memory);
        self.reuse.last_mut().expect("checked").source = Some(held.ty);
        true
    }
}
