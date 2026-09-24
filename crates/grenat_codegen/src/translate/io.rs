//! Output and exit of a standalone program: `puts`, `print`, `p`, `exit`.
//!
//! The runtime prints a value from its bits and a *descriptor* of its type
//! (a string literal of the module, see [`describe`]), exactly as the
//! interpreter prints it.

use cranelift_codegen::ir::{InstBuilder, types};
use grenat_ast::{Expr, ExprKind, FnDef};
use grenat_runtime::status::EXIT;
use grenat_runtime::write::{INSPECT, NEWLINE, PRINT, PUTS};

use super::Translator;
use crate::abi::EXIT_CODE_OFFSET;
use crate::infer::{Method, Typed};
use crate::runtime::Rt;
use crate::structs::Structs;
use crate::ty::Ty;
use crate::walk;

/// The descriptor of `ty`: `I`, `F`, `B`, `S`, `A` then the element's,
/// `R` then `Name(field:descriptor,…)`.
pub(crate) fn describe(ty: Ty, structs: &Structs) -> String {
    match ty {
        Ty::Int => "I".into(),
        Ty::Float => "F".into(),
        Ty::Bool => "B".into(),
        Ty::Str => "S".into(),
        Ty::Array(elem) => format!("A{}", describe(elem.ty().expect("known after inference"), structs)),
        Ty::Struct(id) => {
            let def = structs.get(id);
            let fields: Vec<String> = def.fields.iter().map(|(n, t)| format!("{n}:{}", describe(*t, structs))).collect();
            format!("R{}({})", def.name, fields.join(","))
        }
    }
}

/// The descriptors a function prints (they are literals of the module).
pub(crate) fn descriptors(def: &FnDef, typed: &Typed, structs: &Structs) -> Vec<String> {
    let mut out = Vec::new();
    for stmt in &def.body.stmts {
        walk::each(stmt, &mut |e| {
            if let ExprKind::Call { args, .. } = &e.kind
                && matches!(typed.methods.get(&(e as *const Expr)), Some(Method::Puts | Method::Print | Method::Inspect))
            {
                for arg in args {
                    if let grenat_ast::Arg::Pos(arg) = arg {
                        out.push(describe(typed.ty(arg), structs));
                    }
                }
            }
        });
    }
    out
}

impl Translator<'_, '_> {
    pub(super) fn write(&mut self, method: Method, args: &[&Expr]) {
        let mode = match method {
            Method::Puts => PUTS,
            Method::Print => PRINT,
            _ => INSPECT,
        };
        if args.is_empty() && method == Method::Puts {
            let (ptr, len) = self.literal("");
            let (zero, newline) = (self.b.ins().iconst(types::I64, 0), self.b.ins().iconst(types::I64, NEWLINE));
            self.runtime(Rt::Write, &[zero, ptr, len, newline]);
            return;
        }
        for (i, arg) in args.iter().enumerate() {
            let held = self.operand(arg, &args[i + 1..]);
            let descriptor = describe(held.ty, self.env.structs);
            let (ptr, len) = self.literal(&descriptor);
            let bits = self.slot_bits(held.value, held.ty);
            let mode = self.b.ins().iconst(types::I64, mode);
            self.runtime(Rt::Write, &[bits, ptr, len, mode]);
            self.release(held);
        }
    }

    /// `exit code`: releases everything and leaves every native function up
    /// to the program's entry, which ends the process with `code`.
    pub(super) fn exit(&mut self, code: Option<&Expr>) {
        let code = match code {
            Some(code) => self.scalar(code),
            None => self.b.ins().iconst(types::I64, 0),
        };
        self.store(code, self.ctx, EXIT_CODE_OFFSET);
        let status = self.b.ins().iconst(types::I64, EXIT);
        self.fail(status);
        self.unreachable_block();
    }
}
