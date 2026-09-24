//! Emission of the compiled functions into a Cranelift module: in memory for
//! the JIT, or into an object file for `grenat build`. Both get the same code.

use std::collections::{BTreeSet, HashMap};

use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlagsData, types};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};

use crate::abi::STATUS_OFFSET;
use crate::eligibility::Compiled;
use crate::infer::Signature;
use crate::liveness;
use crate::runtime::Runtime;
use crate::shapes;
use crate::structs::Structs;
use crate::translate::{Callees, Env, Translator};
use crate::ty::Ty;
use crate::walk;

/// What was emitted, for whoever loads the code.
pub(crate) struct Emitted {
    /// The trampoline of each compiled function, in the order of `selected`.
    pub trampolines: Vec<FuncId>,
    /// The shapes, in shape order (see [`shapes`](crate::shapes)).
    pub shapes: Vec<DataId>,
}

/// The target of this machine. Position-independent code for executables.
pub(crate) fn isa(pic: bool) -> Result<OwnedTargetIsa, String> {
    let mut flags = settings::builder();
    let pic = if pic { "true" } else { "false" };
    for (name, value) in [("use_colocated_libcalls", "false"), ("is_pic", pic), ("opt_level", "speed")] {
        flags.set(name, value).map_err(|e| e.to_string())?;
    }
    let isa = cranelift_native::builder()
        .map_err(|e| e.to_string())?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    if isa.pointer_bits() != 64 {
        return Err("native code needs a 64-bit target".into());
    }
    Ok(isa)
}

pub(crate) fn emit(module: &mut impl Module, selected: &[Compiled], structs: &Structs) -> Result<Emitted, String> {
    let fail = |e: cranelift_module::ModuleError| e.to_string();
    let runtime = Runtime::declare(module)?;
    let ids = declare(module, selected)?;

    let shapes = shapes::emit(module, structs)?;

    // every literal once, NUL-terminated (a data object is never empty)
    let texts: BTreeSet<String> = selected
        .iter()
        .flat_map(|c| walk::string_literals(&c.def.body.stmts))
        .chain([String::new()])
        .collect();
    let mut literals = HashMap::new();
    for text in texts {
        let id = module.declare_anonymous_data(false, false).map_err(fail)?;
        let mut data = DataDescription::new();
        data.define([text.as_bytes(), &[0]].concat().into_boxed_slice());
        module.define_data(id, &data).map_err(fail)?;
        literals.insert(text, id);
    }

    let mut builder_ctx = FunctionBuilderContext::new();
    let parts = Parts { selected, ids: &ids, runtime: &runtime, structs, shapes: &shapes, literals: &literals };
    for (index, compiled) in selected.iter().enumerate() {
        define(module, &mut builder_ctx, &parts, index, compiled)?;
    }
    let trampolines = selected
        .iter()
        .enumerate()
        .map(|(i, c)| define_trampoline(module, &mut builder_ctx, ids[i], &c.sig, i))
        .collect::<Result<_, _>>()?;
    Ok(Emitted { trampolines, shapes })
}

fn native_signature(module: &impl Module, sig: &Signature) -> cranelift_codegen::ir::Signature {
    let mut s = module.make_signature();
    for ty in &sig.params {
        s.params.push(AbiParam::new(ty.clif()));
    }
    // recursion depth, the call's context; then (value, status)
    s.params.extend([AbiParam::new(types::I64); 2]);
    s.returns.extend([AbiParam::new(sig.ret.clif()), AbiParam::new(types::I64)]);
    s
}

fn declare(module: &mut impl Module, selected: &[Compiled]) -> Result<Vec<FuncId>, String> {
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
    structs: &'a Structs,
    shapes: &'a [DataId],
    literals: &'a HashMap<String, DataId>,
}

fn define(
    module: &mut impl Module,
    builder_ctx: &mut FunctionBuilderContext,
    parts: &Parts,
    index: usize,
    compiled: &Compiled,
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
    let shapes: Vec<_> = parts.shapes.iter().map(|id| module.declare_data_in_func(*id, &mut ctx.func)).collect();
    let literals = walk::string_literals(&compiled.def.body.stmts)
        .into_iter()
        .chain([String::new()])
        .map(|text| {
            let data = module.declare_data_in_func(parts.literals[&text], &mut ctx.func);
            (text, data)
        })
        .collect();
    let env = Env { callees: &callees, runtime: &runtime, structs: parts.structs, shapes: &shapes, literals: &literals };
    let plan = liveness::plan(compiled.def, &compiled.typed);
    let frontend = module.isa().frontend_config();
    let builder = FunctionBuilder::new(&mut ctx.func, builder_ctx);
    let builder = Translator::function(builder, compiled.def, &compiled.sig, &compiled.typed, &plan, &env);
    builder.finalize(frontend);
    module
        .define_function(parts.ids[index], &mut ctx)
        .map_err(|e| format!("cannot compile `{}`: {e:?}", compiled.def.name.name))
}

/// `extern "C" fn(args, ctx) -> u64`: unpacks the arguments, calls the function
/// at depth 1 with the context, and writes its status to the context.
fn define_trampoline(
    module: &mut impl Module,
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
        values.extend([depth, context]);
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
