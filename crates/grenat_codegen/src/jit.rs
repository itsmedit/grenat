//! Compilation in memory, when a program loads (`grenat run`).

use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use grenat_ast::Program;

use crate::eligibility::select;
use crate::emit::{emit, isa};
use crate::infer::Target;
use crate::native::{Native, Report, Trampoline};
use crate::runtime::symbols;
use crate::structs::Structs;

impl Native {
    /// Compiles every eligible function of `program` into memory.
    pub fn compile(program: &Program) -> Result<Native, String> {
        let structs = Structs::from_program(program);
        let (selected, interpreted) = select(program, &structs, Target::Hosted);
        let report = Report { compiled: selected.iter().map(|c| c.def.name.name.clone()).collect(), interpreted };

        let mut module = module()?;
        let emitted = emit(&mut module, &selected, &structs)?;
        module.finalize_definitions().map_err(|e| e.to_string())?;

        let shapes =
            emitted.shapes.iter().map(|id| module.get_finalized_data(*id).0 as *const grenat_runtime::Shape).collect();

        let trampolines = emitted
            .trampolines
            .iter()
            .map(|id| {
                // SAFETY: the trampoline was emitted with exactly the `Trampoline` signature.
                unsafe { std::mem::transmute::<*const u8, Trampoline>(module.get_finalized_function(*id)) }
            })
            .collect();
        Ok(Native::new(&selected, trampolines, report, structs, shapes, Box::new(module)))
    }
}

/// Address space reserved for the code and the data of one program. Only
/// what is used is backed by memory; a program that needs more is
/// interpreted.
const RESERVATION: usize = 256 << 20;

/// An empty module, linked to the runtime, whose code and data all live in
/// one contiguous reservation.
///
/// The code reaches its data (strings, shapes) and the other functions of the
/// module by pc-relative relocations: ±2 GiB for `adrp` (aarch64) and
/// `lea [rip + disp32]` (x86_64), as cranelift-jit applies them (a GOT,
/// `is_pic`, is x86_64 only in cranelift-jit). By default, cranelift-jit maps
/// code and data in separate mappings, a new one whenever the last is full,
/// where the system finds room: in a process that maps a lot (tests in
/// parallel, a long-running server), a data object and the code using it can
/// be farther apart than that, and finalizing panics. Calls to the runtime go
/// through absolute addresses (`use_colocated_libcalls=false`, imported
/// symbols), so only what the module defines must be near.
pub(crate) fn module() -> Result<JITModule, String> {
    let mut builder = JITBuilder::with_isa(isa(false)?, cranelift_module::default_libcall_names());
    for (name, address) in symbols() {
        builder.symbol(name, address);
    }
    let memory =
        ArenaMemoryProvider::new_with_size(RESERVATION).map_err(|e| format!("no memory for native code: {e}"))?;
    builder.memory_provider(Box::new(memory));
    Ok(JITModule::new(builder))
}

#[cfg(test)]
mod tests {
    use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlagsData, types};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_jit::{ArenaMemoryProvider, JITMemoryKind, JITMemoryProvider};
    use cranelift_module::{DataDescription, Linkage, Module};

    use super::*;

    /// Beyond the reach of a pc-relative relocation, as cranelift-jit applies it.
    const FAR: usize = 2 << 30;

    /// Reserves (without backing) the address space around `address` until
    /// what the system maps next lands farther than [`FAR`]: largest holes
    /// first. Released when the result is dropped.
    fn fill_around(address: usize) -> Vec<ArenaMemoryProvider> {
        let mut reserved = Vec::new();
        for size in (6..=15).rev().map(|half| 1usize << (2 * half)) {
            for _ in 0..4096 {
                let mut region = ArenaMemoryProvider::new_with_size(size).expect("address space");
                let at = region.allocate(1, 1, JITMemoryKind::Writable).expect("reserved") as usize;
                reserved.push(region);
                if at.abs_diff(address) > FAR {
                    break;
                }
            }
        }
        reserved
    }

    /// Code defined after the address space around its data filled up (what
    /// other threads do under load) still reaches the data: it loads a byte
    /// of it through an `adrp`/`add` (aarch64) or a `lea` (x86_64).
    #[test]
    fn code_reaches_its_data_when_the_address_space_around_it_is_taken() {
        let mut module = module().unwrap();
        let data = module.declare_data("answer", Linkage::Local, false, false).unwrap();
        let mut description = DataDescription::new();
        description.define(Box::new([42]));
        module.define_data(data, &description).unwrap();
        let _taken = fill_around(module.get_address(&data.into()) as usize);

        let mut ctx = module.make_context();
        ctx.func.signature.returns.push(AbiParam::new(types::I64));
        let function = module.declare_function("read_answer", Linkage::Local, &ctx.func.signature).unwrap();
        let global = module.declare_data_in_func(data, &mut ctx.func);
        let frontend = module.isa().frontend_config();
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = b.create_block();
        b.switch_to_block(block);
        let address = b.ins().symbol_value(types::I64, global);
        let byte = b.ins().uload8(types::I64, MemFlagsData::trusted(), address, 0);
        b.ins().return_(&[byte]);
        b.seal_all_blocks();
        b.finalize(frontend);
        module.define_function(function, &mut ctx).unwrap();
        module.finalize_definitions().unwrap();

        // SAFETY: the function was defined with exactly this signature.
        let answer: extern "C" fn() -> i64 = unsafe { std::mem::transmute(module.get_finalized_function(function)) };
        assert_eq!(answer(), 42);
    }
}
