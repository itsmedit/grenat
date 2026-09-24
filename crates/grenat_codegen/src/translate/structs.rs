//! Structs: construction (reusing a dying record's memory) and field reads.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{BlockArg, InstBuilder, types};
use grenat_ast::{Arg, Expr};
use grenat_runtime::layout;

use super::{Held, Translator};
use crate::runtime::Rt;
use crate::ty::Ty;

impl Translator<'_, '_> {
    /// `Point(x: …, y: …)`: the fields are evaluated in source order, then
    /// stored into a new record, or into the memory of a record that died
    /// meanwhile (typically `p = Point(x: p.x + 1.0, y: p.y)`).
    pub(super) fn construct(&mut self, e: &Expr, args: &[Arg]) -> Held {
        let construct = &self.typed.constructs[&(e as *const Expr)];
        let (id, slots) = (construct.id, construct.fields.clone());
        let count = self.env.structs.get(id).fields.len();
        let exprs: Vec<&Expr> = args
            .iter()
            .map(|a| match a {
                Arg::Pos(e) | Arg::Named { value: Some(e), .. } => e,
                _ => unreachable!("rejected by infer"),
            })
            .collect();

        self.open_reuse(count);
        let values: Vec<Held> = exprs.iter().enumerate().map(|(i, e)| self.operand(e, &exprs[i + 1..])).collect();
        let token = self.close_reuse();

        let allocate = self.b.create_block();
        let fill = self.b.create_block();
        self.b.append_block_param(fill, types::I64);
        let none = self.b.ins().icmp_imm_s(IntCC::Equal, token, 0);
        self.b.ins().brif(none, allocate, &[], fill, &[BlockArg::Value(token)]);
        self.b.switch_to_block(allocate);
        let fields = self.b.ins().iconst(types::I64, count as i64);
        let fresh = self.runtime(Rt::RecordAlloc, &[fields])[0];
        self.b.ins().jump(fill, &[BlockArg::Value(fresh)]);

        self.b.switch_to_block(fill);
        let record = self.b.block_params(fill)[0];
        let one = self.b.ins().iconst(types::I64, 1);
        self.store(one, record, layout::RC);
        for (held, field) in values.into_iter().zip(slots) {
            let ty = held.ty;
            let value = self.consume(held);
            let bits = self.slot_bits(value, ty);
            self.store(bits, record, layout::field(field));
        }
        Held::object(record, Ty::Struct(id))
    }

    /// `p.x`
    pub(super) fn field(&mut self, recv: Held, index: usize) -> Held {
        let Ty::Struct(id) = recv.ty else { unreachable!("not a struct: {:?}", recv.ty) };
        let ty = self.env.structs.get(id).fields[index].1;
        let bits = self.load(types::I64, recv.value, layout::field(index));
        let value = self.slot_value(bits, ty);
        if ty.is_heap() {
            self.dup(value);
        }
        // the field is not registered yet: releasing the record cannot fail
        self.release(recv);
        Held { value, ty, owned: ty.is_heap() }
    }
}
