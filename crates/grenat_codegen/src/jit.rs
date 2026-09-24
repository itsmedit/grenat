//! Compilation in memory, when a program loads (`grenat run`).

use cranelift_jit::{JITBuilder, JITModule};
use grenat_ast::Program;

use crate::eligibility::select;
use crate::emit::{emit, isa};
use crate::native::{Native, Report, Trampoline};
use crate::runtime::symbols;
use crate::shapes::Shapes;
use crate::structs::Structs;

impl Native {
    /// Compiles every eligible function of `program` into memory.
    pub fn compile(program: &Program) -> Result<Native, String> {
        let structs = Structs::from_program(program);
        let (selected, interpreted) = select(program, &structs);
        let report = Report { compiled: selected.iter().map(|c| c.def.name.name.clone()).collect(), interpreted };

        let mut builder = JITBuilder::with_isa(isa(false)?, cranelift_module::default_libcall_names());
        for (name, address) in symbols() {
            builder.symbol(name, address);
        }
        let mut module = JITModule::new(builder);
        let emitted = emit(&mut module, &selected, &structs)?;
        module.finalize_definitions().map_err(|e| e.to_string())?;

        let shapes = Shapes::build(&structs);
        let (table, size) = module.get_finalized_data(emitted.shapes);
        let pointers = shapes.table();
        assert_eq!(size, pointers.len() * 8, "table of shapes");
        // SAFETY: a writable data object of exactly that size, not yet read by any code
        unsafe {
            std::ptr::copy_nonoverlapping(pointers.as_ptr(), table as *mut *const grenat_runtime::Shape, pointers.len())
        };

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
