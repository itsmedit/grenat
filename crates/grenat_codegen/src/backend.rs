//! Who turns the Cranelift IR of a program into an object file.

use cranelift_module::Module;
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::emit::isa;
use crate::llvm::{LlvmModule, compile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// Fast to build (the default, and the JIT's).
    #[default]
    Cranelift,
    /// The fastest code: LLVM optimizes (`grenat build --release`).
    Llvm,
}

impl Backend {
    /// The object file of what `build` puts into a module, and what it returns.
    pub(crate) fn object<T>(
        self,
        build: impl FnOnce(&mut dyn Module) -> Result<T, String>,
    ) -> Result<(Vec<u8>, T), String> {
        match self {
            Backend::Cranelift => {
                let builder =
                    ObjectBuilder::new(isa(true)?, "grenat_program", cranelift_module::default_libcall_names())
                        .map_err(|e| e.to_string())?;
                let mut module = ObjectModule::new(builder);
                let out = build(&mut module)?;
                Ok((module.finish().emit().map_err(|e| e.to_string())?, out))
            }
            Backend::Llvm => {
                let mut module = LlvmModule::new(isa(true)?);
                let out = build(&mut module)?;
                Ok((compile(&module.finish()?)?, out))
            }
        }
    }
}
