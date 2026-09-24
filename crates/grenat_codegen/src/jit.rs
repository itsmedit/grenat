//! The JIT module: compiles the selected functions and calls them.
//!
//! Every compiled function gets a *trampoline* with one uniform signature,
//! `extern "C" fn(args: *const u64, ctx: *mut Context) -> u64`, so the
//! interpreter can call any of them without knowing its arity or types.
//! This module holds the `unsafe` code of native execution.

use std::collections::HashMap;

use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlagsData, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use grenat_ast::{FnDef, Program};

use crate::abi::{Context, Failure, LIMIT_OFFSET, STATUS_OFFSET};
use crate::data::{Data, Returned};
use crate::eligibility::{Compiled, select};
use crate::infer::Signature;
use crate::liveness;
use crate::marshal::Marshal;
use crate::runtime::{Runtime, symbols};
use crate::shapes::Shapes;
use crate::structs::Structs;
use crate::translate::{Callees, Env, Translator};
use crate::ty::Ty;

type Trampoline = extern "C" fn(*const u64, *mut Context) -> u64;

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
pub struct Jit {
    /// Owns the executable memory the trampolines point into.
    _module: JITModule,
    /// Keyed by the address of the function's definition in the program's AST.
    entries: HashMap<usize, Entry>,
    report: Report,
    structs: Structs,
    /// Shapes and literals whose addresses the code embeds.
    shapes: Shapes,
    _literals: Vec<Box<[u8]>>,
}

// SAFETY: once `finalize_definitions` has run, the module, the shapes and the
// literals are never mutated. Compiled functions only touch their arguments,
// the objects they create, and a caller-owned `Context`: objects never leave
// the thread of the call (values cross the boundary by copy), so concurrent
// calls from several tasks never share state.
unsafe impl Send for Jit {}
unsafe impl Sync for Jit {}

impl Jit {
    /// Compiles every eligible function of `program`.
    pub fn compile(program: &Program) -> Result<Jit, String> {
        let structs = Structs::from_program(program);
        let (selected, interpreted) = select(program, &structs);
        let shapes = Shapes::build(&structs);
        let mut module = new_module()?;
        let runtime = Runtime::declare(&mut module)?;
        let report = Report { compiled: selected.iter().map(|c| c.def.name.name.clone()).collect(), interpreted };

        let ids = declare(&mut module, &selected)?;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut literals = Vec::new();
        for (index, compiled) in selected.iter().enumerate() {
            let parts = Parts { selected: &selected, ids: &ids, runtime: &runtime, shapes: &shapes, structs: &structs };
            define(&mut module, &mut builder_ctx, &parts, index, compiled, &mut literals)?;
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
                (c.def as *const FnDef as usize, Entry { sig: c.sig.clone(), trampoline, mutates: c.mutates })
            })
            .collect();
        Ok(Jit { _module: module, entries, report, structs, shapes, _literals: literals })
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
    pub fn call(&self, def: &FnDef, args: &[Data], depth_limit: usize) -> Option<Result<Returned, Failure>> {
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

        let mut ctx = Context { status: 0, limit: depth_limit as i64 };
        let result = (entry.trampoline)(bits.as_ptr(), &mut ctx);
        let outcome = match ctx.status {
            0 => {
                let ret = entry.sig.ret;
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

fn new_module() -> Result<JITModule, String> {
    let mut flags = settings::builder();
    for (name, value) in [("use_colocated_libcalls", "false"), ("is_pic", "false"), ("opt_level", "speed")] {
        flags.set(name, value).map_err(|e| e.to_string())?;
    }
    let isa = cranelift_native::builder()
        .map_err(|e| e.to_string())?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    if isa.pointer_bits() != 64 {
        return Err("native code needs a 64-bit target".into());
    }
    let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    for (name, address) in symbols() {
        builder.symbol(name, address);
    }
    Ok(JITModule::new(builder))
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

/// What every function's translation shares.
struct Parts<'a, 'p> {
    selected: &'a [Compiled<'p>],
    ids: &'a [FuncId],
    runtime: &'a Runtime,
    shapes: &'a Shapes,
    structs: &'a Structs,
}

fn define(
    module: &mut JITModule,
    builder_ctx: &mut FunctionBuilderContext,
    parts: &Parts,
    index: usize,
    compiled: &Compiled,
    literals: &mut Vec<Box<[u8]>>,
) -> Result<(), String> {
    let mut ctx = module.make_context();
    ctx.func.signature = native_signature(module, &compiled.sig);
    let callees: Callees = parts
        .selected
        .iter()
        .zip(parts.ids)
        .map(|(c, id)| (c.def.name.name.as_str(), module.declare_func_in_func(*id, &mut ctx.func)))
        .collect();
    let runtime = parts.runtime.import(module, &mut ctx.func);
    let env = Env { callees: &callees, runtime: &runtime, shapes: parts.shapes, structs: parts.structs };
    let plan = liveness::plan(compiled.def, &compiled.typed);
    let frontend = module.isa().frontend_config();
    let builder = FunctionBuilder::new(&mut ctx.func, builder_ctx);
    let builder = Translator::function(builder, compiled.def, &compiled.sig, &compiled.typed, &plan, &env, literals);
    builder.finalize(frontend);
    module
        .define_function(parts.ids[index], &mut ctx)
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
            .map(|(i, ty)| {
                let bits = b.ins().load(types::I64, MemFlagsData::trusted(), args, (i * 8) as i32);
                match ty {
                    Ty::Float => b.ins().bitcast(types::F64, MemFlagsData::new(), bits),
                    Ty::Bool => b.ins().ireduce(types::I8, bits),
                    _ => bits,
                }
            })
            .collect();
        let depth = b.ins().iconst(types::I64, 1);
        let limit = b.ins().load(types::I64, MemFlagsData::trusted(), context, LIMIT_OFFSET);
        values.extend([depth, limit]);
        let call = b.ins().call(func, &values);
        let (result, status) = (b.inst_results(call)[0], b.inst_results(call)[1]);
        b.ins().store(MemFlagsData::trusted(), status, context, STATUS_OFFSET);
        let bits = match sig.ret {
            Ty::Float => b.ins().bitcast(types::I64, MemFlagsData::new(), result),
            Ty::Bool => b.ins().uextend(types::I64, result),
            _ => result,
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
