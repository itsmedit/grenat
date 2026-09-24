//! The JIT module: compiles the selected functions and calls them.
//!
//! Every compiled function gets a *trampoline* with one uniform signature,
//! `extern "C" fn(args: *const u64, ctx: *mut Context) -> u64`, so the
//! interpreter can call any of them without knowing its arity or types.
//! This module holds all the `unsafe` code of native execution.

use std::collections::HashMap;

use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlagsData, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use grenat_ast::{FnDef, Program};

use crate::abi::{Context, LIMIT_OFFSET, STATUS_OFFSET, Trap};
use crate::eligibility::{Compiled, select};
use crate::infer::Signature;
use crate::scalar::{Scalar, ScalarTy};
use crate::translate::{Callees, Translator};

type Trampoline = extern "C" fn(*const u64, *mut Context) -> u64;

struct Entry {
    sig: Signature,
    trampoline: Trampoline,
}

/// What was compiled, and why the other candidates were not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub compiled: Vec<String>,
    /// (function, reason)
    pub interpreted: Vec<(String, String)>,
}

/// Native code for the eligible functions of a program.
pub struct Jit {
    /// Owns the executable memory the trampolines point into.
    _module: JITModule,
    /// Keyed by the address of the function's definition in the program's AST.
    entries: HashMap<usize, Entry>,
    report: Report,
}

// SAFETY: once `finalize_definitions` has run, the module is never mutated and
// its code is immutable. Compiled functions are pure: they read their
// arguments and a caller-owned `Context`, so concurrent calls from several
// tasks never share state.
unsafe impl Send for Jit {}
unsafe impl Sync for Jit {}

impl Jit {
    /// Compiles every eligible function of `program`.
    pub fn compile(program: &Program) -> Result<Jit, String> {
        let (selected, interpreted) = select(program);
        let mut module = new_module()?;
        let report = Report { compiled: selected.iter().map(|c| c.def.name.name.clone()).collect(), interpreted };

        let ids = declare(&mut module, &selected)?;
        let mut builder_ctx = FunctionBuilderContext::new();
        for (index, compiled) in selected.iter().enumerate() {
            define(&mut module, &mut builder_ctx, &selected, &ids, index, compiled)?;
        }
        let trampolines: Vec<FuncId> = selected
            .iter()
            .enumerate()
            .map(|(i, c)| define_trampoline(&mut module, &mut builder_ctx, ids[i], &c.sig, i))
            .collect::<Result<_, _>>()?;
        module.finalize_definitions().map_err(|e| e.to_string())?;

        let entries = selected
            .iter()
            .zip(trampolines)
            .map(|(c, id)| {
                // SAFETY: the trampoline was built above with exactly the `Trampoline` signature.
                let trampoline =
                    unsafe { std::mem::transmute::<*const u8, Trampoline>(module.get_finalized_function(id)) };
                (c.def as *const FnDef as usize, Entry { sig: c.sig.clone(), trampoline })
            })
            .collect();
        Ok(Jit { _module: module, entries, report })
    }

    pub fn report(&self) -> &Report {
        &self.report
    }

    pub fn is_compiled(&self, def: &FnDef) -> bool {
        self.entries.contains_key(&(def as *const FnDef as usize))
    }

    /// Calls the native version of `def`. `None` if it is not compiled or if
    /// the arguments' types do not match its signature exactly (the caller
    /// then interprets the call, which keeps the semantics identical).
    pub fn call(&self, def: &FnDef, args: &[Scalar], depth_limit: usize) -> Option<Result<Scalar, Trap>> {
        let entry = self.entries.get(&(def as *const FnDef as usize))?;
        let matches =
            args.len() == entry.sig.params.len() && args.iter().zip(&entry.sig.params).all(|(a, t)| a.ty() == *t);
        if !matches {
            return None;
        }
        let bits: Vec<u64> = args.iter().map(|a| a.to_bits()).collect();
        let mut ctx = Context { status: 0, limit: depth_limit as i64 };
        let result = (entry.trampoline)(bits.as_ptr(), &mut ctx);
        Some(match ctx.status {
            0 => Ok(Scalar::from_bits(entry.sig.ret, result)),
            status => Err(Trap::from_status(status)),
        })
    }
}

fn new_module() -> Result<JITModule, String> {
    let mut flags = settings::builder();
    for (name, value) in [("use_colocated_libcalls", "false"), ("is_pic", "false"), ("opt_level", "speed")] {
        flags.set(name, value).map_err(|e| e.to_string())?;
    }
    let isa = cranelift_native::builder()
        .map_err(|e| e.to_string())?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    Ok(JITModule::new(JITBuilder::with_isa(isa, cranelift_module::default_libcall_names())))
}

fn native_signature(module: &JITModule, sig: &Signature) -> cranelift_codegen::ir::Signature {
    let mut s = module.make_signature();
    for ty in &sig.params {
        s.params.push(AbiParam::new(ty.clif()));
    }
    // recursion depth and its limit, then (value, status)
    s.params.extend([AbiParam::new(types::I64), AbiParam::new(types::I64)]);
    s.returns.extend([AbiParam::new(sig.ret.clif()), AbiParam::new(types::I64)]);
    s
}

fn declare(module: &mut JITModule, selected: &[Compiled]) -> Result<Vec<FuncId>, String> {
    selected
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let sig = native_signature(module, &c.sig);
            module.declare_function(&symbol("fn", i, &c.def.name.name), Linkage::Local, &sig).map_err(|e| e.to_string())
        })
        .collect()
}

fn define(
    module: &mut JITModule,
    builder_ctx: &mut FunctionBuilderContext,
    selected: &[Compiled],
    ids: &[FuncId],
    index: usize,
    compiled: &Compiled,
) -> Result<(), String> {
    let mut ctx = module.make_context();
    ctx.func.signature = native_signature(module, &compiled.sig);
    let callees: Callees = selected
        .iter()
        .zip(ids)
        .map(|(c, id)| (c.def.name.name.as_str(), module.declare_func_in_func(*id, &mut ctx.func)))
        .collect();
    let frontend = module.isa().frontend_config();
    let builder = FunctionBuilder::new(&mut ctx.func, builder_ctx);
    let builder = Translator::function(builder, compiled.def, &compiled.sig, &compiled.typed, &callees);
    builder.finalize(frontend);
    module
        .define_function(ids[index], &mut ctx)
        .map_err(|e| format!("cannot compile `{}`: {e:?}", compiled.def.name.name))
}

/// `extern "C" fn(args, ctx) -> u64`: unpacks the arguments, calls the function
/// at depth 1 with the context's limit, and writes its status to the context.
fn define_trampoline(
    module: &mut JITModule,
    builder_ctx: &mut FunctionBuilderContext,
    target: FuncId,
    sig: &Signature,
    index: usize,
) -> Result<FuncId, String> {
    let ptr = module.target_config().pointer_type();
    let mut ctx = module.make_context();
    ctx.func.signature.params.extend([AbiParam::new(ptr), AbiParam::new(ptr)]);
    ctx.func.signature.returns.push(AbiParam::new(types::I64));
    let id = module
        .declare_function(&symbol("trampoline", index, ""), Linkage::Local, &ctx.func.signature)
        .map_err(|e| e.to_string())?;
    let func = module.declare_func_in_func(target, &mut ctx.func);
    let frontend = module.isa().frontend_config();
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, builder_ctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let (args, context) = (b.block_params(entry)[0], b.block_params(entry)[1]);
        let mut values: Vec<_> = sig
            .params
            .iter()
            .enumerate()
            .map(|(i, ty)| b.ins().load(ty.clif(), MemFlagsData::trusted(), args, (i * 8) as i32))
            .collect();
        let depth = b.ins().iconst(types::I64, 1);
        let limit = b.ins().load(types::I64, MemFlagsData::trusted(), context, LIMIT_OFFSET);
        values.extend([depth, limit]);
        let call = b.ins().call(func, &values);
        let (result, status) = (b.inst_results(call)[0], b.inst_results(call)[1]);
        b.ins().store(MemFlagsData::trusted(), status, context, STATUS_OFFSET);
        let bits = match sig.ret {
            ScalarTy::Int => result,
            ScalarTy::Float => b.ins().bitcast(types::I64, MemFlagsData::new(), result),
            ScalarTy::Bool => b.ins().uextend(types::I64, result),
        };
        b.ins().return_(&[bits]);
        b.seal_all_blocks();
        b.finalize(frontend);
    }
    module.define_function(id, &mut ctx).map_err(|e| format!("cannot compile a trampoline: {e:?}"))?;
    Ok(id)
}

/// Unique, assembler-safe symbol name (`even?` is not a valid symbol).
fn symbol(kind: &str, index: usize, name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("grenat_{kind}_{index}_{clean}")
}
