//! Static checking for Grenat: names, types, effects and the `~T` taint.
//!
//! The checker is **gradual**: whatever it cannot type becomes
//! `Ty::Unknown`, which is compatible with everything and never produces an
//! error. What it does prove, it proves before execution:
//!
//! - **taint**: an untrusted value (a model's answer, a network response) cannot reach a function with a
//!   dangerous effect (`shell`, `net`, `fs.write`, `human`) without
//!   `.check`, `.approve(by: :human)` or `.trust!` (E0412); the analysis
//!   follows taint through calls (each function is checked for the actual
//!   taint of its arguments), fields, interpolation, blocks and agent
//!   `@…` state;
//! - **effects**: a function that declares `uses` must cover everything its
//!   body does, and both `main` and `tool`s must declare theirs (E0300);
//! - **names and types**: variables, fields, methods, arity, named
//!   arguments, incompatible types (E0100, E0200);
//! - **declarations**: well-formed `prompt`s, agents and tools (E0413, E0500).
//!
//! The interpreter keeps its runtime checks: defense in depth.

mod agents;
mod builtins;
mod calls;
mod checker;
mod collect;
mod construct;
mod context;
mod declarations;
mod effects;
mod expr;
mod functions;
mod methods;
mod names;
mod ops;
mod pattern;
mod resolve;
mod secrets;
mod suggest;
mod taint;
mod ty;

pub use grenat_ast::Diagnostic;
use grenat_ast::Program;

pub(crate) use checker::*;
pub(crate) use context::*;
pub(crate) use effects::*;
pub(crate) use names::*;
pub(crate) use suggest::*;
pub(crate) use taint::*;

pub const E_NAME: &str = "E0100";
pub const E_TYPE: &str = "E0200";
pub const E_EFFECT: &str = "E0300";
pub const E_WORKFLOW: &str = "E0310";
pub const E_TAINT: &str = "E0412";
pub const E_TAINT_DECL: &str = "E0413";
pub const E_SECRET: &str = "E0414";
pub const E_DECL: &str = "E0500";

/// Checks a program; returns the diagnostics sorted by position.
pub fn check(program: &Program) -> Vec<Diagnostic> {
    let mut checker = Checker::new(program);
    checker.collect();
    checker.infer_ivars();
    // tainted `@…` state and the effects of recursive functions propagate from one pass to the next
    for _ in 0..4 {
        let before = (checker.ivar_taint.len(), checker.effect_count());
        checker.memo.clear();
        checker.pass();
        if (checker.ivar_taint.len(), checker.effect_count()) == before {
            break;
        }
    }
    let mut diags = checker.diags;
    diags.sort_by_key(|d| (d.span.start, d.span.end));
    diags
}
