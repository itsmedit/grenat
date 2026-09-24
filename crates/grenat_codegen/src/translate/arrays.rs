//! Arrays: literals, elements, methods and loops over them.
//!
//! Elements are read and written inline. Whatever the interpreter answers
//! with `nil` or by growing the array with `nil`s (an index out of range, the
//! first element of an empty array…) deoptimizes.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{BlockArg, InstBuilder, MemFlagsData, Value, types};
use grenat_ast::{Block, Expr};
use grenat_runtime::layout;

use super::{Held, Translator};
use crate::abi::Trap;
use crate::infer::Method;
use crate::liveness::Edge;
use crate::runtime::Rt;
use crate::ty::{Elem, Ty};

fn elem_of(ty: Ty) -> Ty {
    match ty {
        Ty::Array(elem) => elem.ty().expect("known after inference"),
        other => unreachable!("not an array: {other:?}"),
    }
}

impl Translator<'_, '_> {
    // ── Slots ────────────────────────────────────────────────

    /// A value as the 64 bits of a slot.
    pub(super) fn slot_bits(&mut self, value: Value, ty: Ty) -> Value {
        match ty {
            Ty::Float => self.b.ins().bitcast(types::I64, MemFlagsData::new(), value),
            Ty::Bool => self.b.ins().uextend(types::I64, value),
            _ => value,
        }
    }

    pub(super) fn slot_value(&mut self, bits: Value, ty: Ty) -> Value {
        match ty {
            Ty::Float => self.b.ins().bitcast(types::F64, MemFlagsData::new(), bits),
            Ty::Bool => self.b.ins().ireduce(types::I8, bits),
            _ => bits,
        }
    }

    /// Address of slot `index` of array `a` (in bounds).
    fn slot_address(&mut self, a: Value, index: Value) -> Value {
        let data = self.load(types::I64, a, layout::DATA);
        let offset = self.b.ins().imul_imm_s(index, i64::from(layout::SLOT));
        self.b.ins().iadd(data, offset)
    }

    /// Element `index` of a live array, as an owned value (in bounds).
    fn load_element(&mut self, a: Value, index: Value, ty: Ty) -> Held {
        let addr = self.slot_address(a, index);
        let bits = self.load(types::I64, addr, 0);
        let value = self.slot_value(bits, ty);
        if ty.is_heap() {
            self.dup(value);
        }
        Held { value, ty, owned: ty.is_heap() }
    }

    fn length(&mut self, a: Value) -> Value {
        self.load(types::I64, a, layout::LEN)
    }

    /// `i`, or `i + len` for a negative index.
    fn resolve_index(&mut self, index: Value, len: Value) -> Value {
        let negative = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
        let from_end = self.b.ins().iadd(index, len);
        self.b.ins().select(negative, from_end, index)
    }

    // ── Elements ─────────────────────────────────────────────

    /// `[a, b, c]`
    pub(super) fn array_literal(&mut self, e: &Expr, items: &[Expr]) -> Held {
        let ty = self.typed.ty(e);
        let capacity = self.b.ins().iconst(types::I64, items.len() as i64);
        let a = self.runtime(Rt::ArrayNew, &[capacity])[0];
        self.hold(Held::object(a, ty));
        for item in items {
            let held = self.eval(item).expect("a value");
            let value = self.consume(held);
            let bits = self.slot_bits(value, held.ty);
            self.runtime(Rt::ArrayPush, &[a, bits]);
        }
        self.take(a);
        Held::object(a, ty)
    }

    /// `xs[i]`, not yet registered as pending.
    pub(super) fn element(&mut self, array: Held, index: Value) -> Held {
        let len = self.length(array.value);
        let i = self.resolve_index(index, len);
        // unsigned: a still negative index is huge
        let out = self.b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, i, len);
        self.deopt_if(out);
        self.load_element(array.value, i, elem_of(array.ty))
    }

    /// `xs[i] = item`: replaces an element, or appends at `i == length`.
    pub(super) fn store_element(&mut self, array: Held, index: Value, item: Held) {
        let a = array.value;
        let len = self.length(a);
        let i = self.resolve_index(index, len);
        let negative = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, i, 0);
        let beyond = self.b.ins().icmp(IntCC::SignedGreaterThan, i, len);
        // the interpreter raises `IndexError`, or fills the gap with `nil`s
        let out = self.b.ins().bor(negative, beyond);
        self.deopt_if(out);
        let ty = item.ty;
        let value = self.consume(item);
        let bits = self.slot_bits(value, ty);

        let append = self.b.create_block();
        let replace = self.b.create_block();
        let done = self.b.create_block();
        let at_end = self.b.ins().icmp(IntCC::Equal, i, len);
        self.b.ins().brif(at_end, append, &[], replace, &[]);
        self.b.switch_to_block(append);
        self.runtime(Rt::ArrayPush, &[a, bits]);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(replace);
        let addr = self.slot_address(a, i);
        let old = self.load(types::I64, addr, 0);
        self.store(bits, addr, 0);
        if ty.is_heap() {
            self.drop_ref(old, ty);
        }
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// `xs << item`, `xs.push(item)`
    pub(super) fn push(&mut self, recv: &Expr, item: &Expr) {
        let array = self.operand(recv, &[item]);
        let held = self.operand(item, &[]);
        let ty = held.ty;
        let value = self.consume(held);
        let bits = self.slot_bits(value, ty);
        self.runtime(Rt::ArrayPush, &[array.value, bits]);
        self.release(array);
    }

    /// `l + r`: a new array.
    pub(super) fn array_concat(&mut self, l: Held, r: Held) -> Held {
        let heap = self.heap_flag(l.ty);
        let a = self.runtime(Rt::ArrayConcat, &[l.value, r.value, heap])[0];
        self.release(l);
        self.release(r);
        Held::object(a, l.ty)
    }

    fn heap_flag(&mut self, array: Ty) -> Value {
        self.b.ins().iconst(types::I64, i64::from(matches!(array, Ty::Array(e) if e.is_heap())))
    }

    // ── Methods ──────────────────────────────────────────────

    pub(super) fn array_method(&mut self, method: Method, array: Held) -> Held {
        let a = array.value;
        let elem = match array.ty {
            Ty::Array(e) => e,
            other => unreachable!("not an array: {other:?}"),
        };
        let result = match method {
            Method::Length => Held::scalar(self.length(a), Ty::Int),
            Method::Empty => {
                let len = self.length(a);
                Held::scalar(self.b.ins().icmp_imm_s(IntCC::Equal, len, 0), Ty::Bool)
            }
            Method::First | Method::Last => {
                let at = self.b.ins().iconst(types::I64, if method == Method::First { 0 } else { -1 });
                self.element(array, at)
            }
            Method::Pop => {
                let len = self.length(a);
                let empty = self.b.ins().icmp_imm_s(IntCC::Equal, len, 0);
                self.deopt_if(empty);
                let last = self.b.ins().iadd_imm_s(len, -1);
                self.store(last, a, layout::LEN);
                // the array's reference moves to the result
                let addr = self.slot_address(a, last);
                let bits = self.load(types::I64, addr, 0);
                let ty = elem.ty().expect("known");
                let value = self.slot_value(bits, ty);
                Held { value, ty, owned: ty.is_heap() }
            }
            Method::Sum => self.sum(a, elem),
            Method::Copy => {
                let heap = self.heap_flag(array.ty);
                Held::object(self.runtime(Rt::ArrayCopy, &[a, heap])[0], array.ty)
            }
            other => unreachable!("not an array method: {other:?}"),
        };
        // the result is not registered yet: releasing the array cannot fail
        self.release(array);
        result
    }

    /// `xs.sum`: `0 + x0 + x1…` as the interpreter adds them (checked for
    /// `Int`; for `Float`, the empty sum is the `Int` 0: deoptimized).
    fn sum(&mut self, a: Value, elem: Elem) -> Held {
        let ty = elem.ty().expect("known");
        let len = self.length(a);
        if ty == Ty::Float {
            let empty = self.b.ins().icmp_imm_s(IntCC::Equal, len, 0);
            self.deopt_if(empty);
        }
        let header = self.b.create_block();
        let body = self.b.create_block();
        let done = self.b.create_block();
        self.b.append_block_param(header, types::I64);
        self.b.append_block_param(header, ty.clif());
        self.b.append_block_param(done, ty.clif());
        let zero = self.b.ins().iconst(types::I64, 0);
        let start = self.zero(ty);
        self.b.ins().jump(header, &[BlockArg::Value(zero), BlockArg::Value(start)]);

        self.b.switch_to_block(header);
        let (i, acc) = (self.b.block_params(header)[0], self.b.block_params(header)[1]);
        let more = self.b.ins().icmp(IntCC::SignedLessThan, i, len);
        self.b.ins().brif(more, body, &[], done, &[BlockArg::Value(acc)]);

        self.b.switch_to_block(body);
        let addr = self.slot_address(a, i);
        let bits = self.load(types::I64, addr, 0);
        let x = self.slot_value(bits, ty);
        let next = if ty == Ty::Int {
            let (sum, overflow) = self.b.ins().sadd_overflow(acc, x);
            self.trap_if(overflow, Trap::Overflow as i64);
            sum
        } else {
            self.b.ins().fadd(acc, x)
        };
        let i = self.b.ins().iadd_imm_s(i, 1);
        self.b.ins().jump(header, &[BlockArg::Value(i), BlockArg::Value(next)]);

        self.b.switch_to_block(done);
        Held::scalar(self.b.block_params(done)[0], ty)
    }

    // ── Loops ────────────────────────────────────────────────

    /// `xs.each do |x| … end` (and `each_with_index`): iterates over a copy
    /// taken first, as the interpreter iterates over a snapshot.
    pub(super) fn each(&mut self, call: &Expr, recv: &Expr, block: &Block, with_index: bool) {
        let array = self.operand(recv, &[]);
        let heap = self.heap_flag(array.ty);
        let snapshot = self.runtime(Rt::ArrayCopy, &[array.value, heap])[0];
        let snapshot = self.hold(Held::object(snapshot, array.ty));
        self.release(array);
        let elem = elem_of(snapshot.ty);
        let s = snapshot.value;
        self.counted_loop(call, block, None, move |t, i| {
            let x = t.load_element(s, i, elem);
            let mut params = vec![t.hold(x)];
            if with_index {
                params.push(Held::scalar(i, Ty::Int));
            }
            params
        }, Some(s));
        self.release(snapshot);
    }

    /// `n.times do |i|`, `a.upto(b) do |i|`
    pub(super) fn count(&mut self, call: &Expr, from: Value, to: Value, inclusive: bool, block: &Block) {
        self.counted_loop(call, block, Some((from, to, inclusive)), |_, i| vec![Held::scalar(i, Ty::Int)], None);
    }

    /// A loop over `i`, whose body binds the block's parameters then runs
    /// the block. `range`: `i` from `from` to `to`; otherwise `i` covers the
    /// indexes of array `over`.
    fn counted_loop(
        &mut self,
        call: &Expr,
        block: &Block,
        range: Option<(Value, Value, bool)>,
        mut params: impl FnMut(&mut Self, Value) -> Vec<Held>,
        over: Option<Value>,
    ) {
        let header = self.b.create_block();
        let body = self.b.create_block();
        let latch = self.b.create_block();
        let exit = self.b.create_block();
        self.b.append_block_param(header, types::I64);
        self.b.append_block_param(latch, types::I64);
        let start = match range {
            Some((from, ..)) => from,
            None => self.b.ins().iconst(types::I64, 0),
        };
        self.b.ins().jump(header, &[BlockArg::Value(start)]);

        self.b.switch_to_block(header);
        let at_header = self.owned.clone();
        let i = self.b.block_params(header)[0];
        let more = match (range, over) {
            (Some((_, to, true)), _) => self.b.ins().icmp(IntCC::SignedLessThanOrEqual, i, to),
            (Some((_, to, false)), _) => self.b.ins().icmp(IntCC::SignedLessThan, i, to),
            (None, Some(array)) => {
                let len = self.length(array);
                self.b.ins().icmp(IntCC::SignedLessThan, i, len)
            }
            (None, None) => unreachable!("a range or an array"),
        };
        self.b.ins().brif(more, body, &[], exit, &[]);

        self.b.switch_to_block(body);
        self.edge(call, Edge::Body);
        let names = self.typed.blocks[&(call as *const Expr)].params.clone();
        let values = params(self, i);
        for (name, held) in names.iter().zip(values) {
            self.define(name, held, None);
        }
        for name in self.plan.dead_params(call).to_vec() {
            self.drop_var(&name);
        }
        self.loops += 1;
        if let Some(held) = self.stmts(&block.body.stmts) {
            self.release(held);
        }
        self.loops -= 1;
        debug_assert_eq!(self.owned, at_header, "a block changes ownership");
        self.checkpoint();
        self.b.ins().jump(latch, &[BlockArg::Value(i)]);

        // `upto` stops at its bound before incrementing: no overflow at `Int::MAX`
        self.b.switch_to_block(latch);
        let i = self.b.block_params(latch)[0];
        match range {
            Some((_, to, true)) => {
                let last = self.b.ins().icmp(IntCC::Equal, i, to);
                let next_block = self.b.create_block();
                self.b.ins().brif(last, exit, &[], next_block, &[]);
                self.b.switch_to_block(next_block);
                let next = self.b.ins().iadd_imm_s(i, 1);
                self.b.ins().jump(header, &[BlockArg::Value(next)]);
            }
            _ => {
                let next = self.b.ins().iadd_imm_s(i, 1);
                self.b.ins().jump(header, &[BlockArg::Value(next)]);
            }
        }

        self.b.switch_to_block(exit);
        self.owned = at_header;
        self.edge(call, Edge::Exit);
    }
}
