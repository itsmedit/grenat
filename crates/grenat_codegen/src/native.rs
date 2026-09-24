//! Loaded native code: the compiled functions of a program, and how the
//! interpreter calls them. Built by the JIT ([`Native::compile`]) or from the
//! code linked into an executable ([`Native::link`]).
//!
//! Every compiled function has a *trampoline* with one uniform signature,
//! `extern "C" fn(args: *const u64, ctx: *mut Context) -> u64`, so the
//! interpreter can call any of them without knowing its arity or types.

use std::any::Any;
use std::collections::HashMap;

use grenat_ast::FnDef;

use crate::abi::{Context, Failure};
use crate::data::{Data, Returned};
use crate::eligibility::Compiled;
use crate::infer::Signature;
use crate::marshal::Marshal;
use crate::structs::Structs;
use crate::ty::Ty;

pub(crate) use grenat_runtime::abi::Trampoline;

struct Entry {
    sig: Signature,
    trampoline: Trampoline,
    /// May modify its array arguments: they are read back after a call.
    mutates: bool,
}

/// What was compiled, and why the other candidates were not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub compiled: Vec<String>,
    /// (function, reason)
    pub interpreted: Vec<(String, String)>,
}

/// Native code for the eligible functions of a program.
pub struct Native {
    /// Keyed by the address of the function's definition in the program's AST.
    entries: HashMap<usize, Entry>,
    report: Report,
    structs: Structs,
    /// Addresses of the shapes (data of the code), in shape order.
    shapes: Vec<*const grenat_runtime::Shape>,
    /// Keeps the code alive (the JIT's memory; nothing for linked code).
    _code: Box<dyn Any>,
}

// SAFETY: the code and the shapes are never mutated once loaded. Compiled functions only touch their arguments, the objects they
// create, and a caller-owned `Context`: objects never leave the thread of the
// call (values cross the boundary by copy), so concurrent calls from several
// tasks never share state.
unsafe impl Send for Native {}
unsafe impl Sync for Native {}

impl Native {
    /// Assembles loaded code: `trampolines` in the order of `selected`.
    pub(crate) fn new(
        selected: &[Compiled],
        trampolines: Vec<Trampoline>,
        report: Report,
        structs: Structs,
        shapes: Vec<*const grenat_runtime::Shape>,
        code: Box<dyn Any>,
    ) -> Native {
        let entries = selected
            .iter()
            .zip(trampolines)
            .map(|(c, trampoline)| {
                (c.def as *const FnDef as usize, Entry { sig: c.sig.clone(), trampoline, mutates: c.mutates })
            })
            .collect();
        Native { entries, report, structs, shapes, _code: code }
    }

    pub fn report(&self) -> &Report {
        &self.report
    }

    pub fn is_compiled(&self, def: &FnDef) -> bool {
        self.entries.contains_key(&(def as *const FnDef as usize))
    }

    /// Calls the native version of `def`. `None` if it is not compiled or if
    /// the arguments do not have exactly its parameter types (the caller then
    /// interprets the call, which keeps the semantics identical).
    ///
    /// `cancelled` is asked regularly while native code runs (every
    /// `grenat_runtime::POLL_TICK`): `true` stops the call with
    /// [`Trap::Cancelled`](crate::Trap::Cancelled).
    pub fn call(
        &self,
        def: &FnDef,
        args: &[Data],
        depth_limit: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Option<Result<Returned, Failure>> {
        let entry = self.entries.get(&(def as *const FnDef as usize))?;
        let params = &entry.sig.params;
        let marshal = Marshal { structs: &self.structs, shapes: &self.shapes };
        let fits = args.len() == params.len()
            && args.iter().zip(params).enumerate().all(|(i, (arg, ty))| match arg {
                Data::Alias(j) => *j < i && params[*j] == *ty && matches!(args[*j], Data::Array(_)),
                arg => marshal.fits(arg, *ty),
            });
        if !fits {
            return None;
        }

        // every argument is a new reference handed over to the function;
        // arrays get a second one, kept to read them back afterwards
        let mut bits: Vec<u64> = Vec::with_capacity(args.len());
        for (arg, ty) in args.iter().zip(params) {
            let value = match arg {
                Data::Alias(j) => {
                    marshal.retain(bits[*j]);
                    bits[*j]
                }
                arg => marshal.write(arg, *ty),
            };
            bits.push(value);
        }
        let kept: Vec<usize> = (0..args.len())
            .filter(|&i| matches!(params[i], Ty::Array(_)) && !matches!(args[i], Data::Alias(_)))
            .collect();
        for &i in &kept {
            marshal.retain(bits[i]);
        }

        let mut ctx = Context {
            status: 0,
            limit: depth_limit as i64,
            site: 0,
            exit_code: 0,
            poll: grenat_runtime::Poll::new(cancelled),
        };
        let ctx_ptr: *mut Context = &mut ctx;
        // SAFETY: `ctx` stays in place for the whole call
        let result = grenat_runtime::polled(unsafe { &(*ctx_ptr).poll }, || (entry.trampoline)(bits.as_ptr(), ctx_ptr));
        let outcome = match ctx.status {
            0 => {
                let ret = entry.sig.ret.expect("hosted functions return a value");
                let value = match kept.iter().find(|&&i| bits[i] == result) {
                    Some(&i) if matches!(ret, Ty::Array(_)) => Data::Alias(i),
                    _ => marshal.read(result, ret),
                };
                marshal.release(result, ret);
                let mut arrays = vec![None; args.len()];
                for &i in kept.iter().filter(|_| entry.mutates) {
                    let Ty::Array(elem) = params[i] else { unreachable!("kept arrays") };
                    arrays[i] = Some(marshal.items(bits[i], elem));
                }
                Ok(Returned { value, arrays })
            }
            status => Err(Failure::from_status(status)),
        };
        for &i in &kept {
            marshal.release(bits[i], params[i]);
        }
        Some(outcome)
    }
}
