//! Native code generation for Grenat, with Cranelift.
//!
//! A top-level `def` whose parameters and return value have native types
//! (`Int`, `Float`, `Bool`, `String`, `Array(T)`, structs of those) and
//! whose body stays within the compiled subset (arithmetic, strings, arrays,
//! structs, `if`, `while`, `return`, loops over blocks, calls to other such
//! functions) is compiled to machine code when the program loads.
//!
//! Objects are reference counted, Perceus style: the compiler inserts every
//! `dup` and `drop` from the liveness of variables, appends in place to a
//! string it owns uniquely, and builds a new record in the memory of one that
//! just died. There is no garbage collector and no leak, even on errors.
//!
//! The interpreter calls the native version when the actual arguments have
//! exactly the declared types, and interprets the call otherwise; native
//! code that meets something it cannot represent (`nil`) *deoptimizes*: the
//! interpreter runs the call again. Results and errors are identical.
//!
//! The same code is emitted in memory when a program loads ([`Native::compile`],
//! the JIT) or into an object file linked into an executable ([`aot`],
//! `grenat build`).

mod abi;
pub mod aot;
mod data;
mod eligibility;
mod emit;
mod infer;
mod jit;
mod liveness;
mod marshal;
mod native;
mod runtime;
mod shapes;
mod structs;
mod translate;
mod ty;
mod walk;

pub use abi::{Failure, Trap};
pub use data::{Data, Returned};
pub use native::{Native, Report};
