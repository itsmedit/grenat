//! Native code generation for Grenat, with Cranelift.
//!
//! First slice of phase 4: a JIT for pure numeric functions. A top-level
//! `def` whose parameters and return value are annotated `Int`, `Float` or
//! `Bool`, and whose body only uses arithmetic, comparisons, locals, `if`,
//! `while`, `return` and calls to other such functions, is compiled to
//! machine code when the program loads. The interpreter calls the native
//! version when the actual arguments have exactly the declared types, and
//! interprets the call otherwise: results and errors are identical.
//!
//! Next slices: strings, arrays and structs with reference counting, then
//! ahead-of-time compilation to an executable (`grenat build`).

mod abi;
mod eligibility;
mod infer;
mod jit;
mod scalar;
mod translate;

pub use abi::Trap;
pub use infer::Signature;
pub use jit::{Jit, Report};
pub use scalar::{Scalar, ScalarTy};
