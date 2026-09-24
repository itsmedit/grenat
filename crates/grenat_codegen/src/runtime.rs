//! The runtime functions compiled code calls (see `grenat_runtime`).

use std::collections::HashMap;

use cranelift_codegen::ir::{AbiParam, FuncRef, Function, Type, types};
use cranelift_module::{FuncId, Linkage, Module};

/// A function of `grenat_runtime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Rt {
    Poll,
    Write,
    Free,
    DropReuse,
    FreeToken,
    RecordAlloc,
    ArrayNew,
    ArrayPush,
    ArrayConcat,
    ArrayCopy,
    StrFrom,
    StrConcat,
    StrAddOwned,
    StrPushBytes,
    StrPushStr,
    StrPushInt,
    StrPushFloat,
    StrPushBool,
    StrEq,
    StrCmp,
    StrLength,
    StrCharAt,
    StrUpcase,
    StrDowncase,
    StrReverse,
    StrStrip,
    StrRepeat,
    StrIncludes,
    StrStartsWith,
    StrEndsWith,
    StrToI,
    StrToF,
}

const I: Type = types::I64;
const F: Type = types::F64;

impl Rt {
    const ALL: [Rt; 32] = [
        Rt::Poll,
        Rt::Write,
        Rt::Free,
        Rt::DropReuse,
        Rt::FreeToken,
        Rt::RecordAlloc,
        Rt::ArrayNew,
        Rt::ArrayPush,
        Rt::ArrayConcat,
        Rt::ArrayCopy,
        Rt::StrFrom,
        Rt::StrConcat,
        Rt::StrAddOwned,
        Rt::StrPushBytes,
        Rt::StrPushStr,
        Rt::StrPushInt,
        Rt::StrPushFloat,
        Rt::StrPushBool,
        Rt::StrEq,
        Rt::StrCmp,
        Rt::StrLength,
        Rt::StrCharAt,
        Rt::StrUpcase,
        Rt::StrDowncase,
        Rt::StrReverse,
        Rt::StrStrip,
        Rt::StrRepeat,
        Rt::StrIncludes,
        Rt::StrStartsWith,
        Rt::StrEndsWith,
        Rt::StrToI,
        Rt::StrToF,
    ];

    /// Symbol, parameter types, result types. Pointers and booleans are `i64`.
    fn info(self) -> (&'static str, &'static [Type], &'static [Type]) {
        match self {
            Rt::Poll => ("grenat_poll", &[I], &[I]),
            Rt::Write => ("grenat_write", &[I, I, I, I], &[]),
            Rt::Free => ("grenat_free", &[I, I], &[]),
            Rt::DropReuse => ("grenat_drop_reuse", &[I, I], &[I]),
            Rt::FreeToken => ("grenat_free_token", &[I, I], &[]),
            Rt::RecordAlloc => ("grenat_record_alloc", &[I], &[I]),
            Rt::ArrayNew => ("grenat_array_new", &[I], &[I]),
            Rt::ArrayPush => ("grenat_array_push", &[I, I], &[]),
            Rt::ArrayConcat => ("grenat_array_concat", &[I, I, I], &[I]),
            Rt::ArrayCopy => ("grenat_array_copy", &[I, I], &[I]),
            Rt::StrFrom => ("grenat_str_from", &[I, I], &[I]),
            Rt::StrConcat => ("grenat_str_concat", &[I, I], &[I]),
            Rt::StrAddOwned => ("grenat_str_add_owned", &[I, I], &[I]),
            Rt::StrPushBytes => ("grenat_str_push_bytes", &[I, I, I], &[]),
            Rt::StrPushStr => ("grenat_str_push_str", &[I, I], &[]),
            Rt::StrPushInt => ("grenat_str_push_int", &[I, I], &[]),
            Rt::StrPushFloat => ("grenat_str_push_float", &[I, F], &[]),
            Rt::StrPushBool => ("grenat_str_push_bool", &[I, I], &[]),
            Rt::StrEq => ("grenat_str_eq", &[I, I], &[I]),
            Rt::StrCmp => ("grenat_str_cmp", &[I, I], &[I]),
            Rt::StrLength => ("grenat_str_length", &[I], &[I]),
            Rt::StrCharAt => ("grenat_str_char_at", &[I, I], &[I]),
            Rt::StrUpcase => ("grenat_str_upcase", &[I], &[I]),
            Rt::StrDowncase => ("grenat_str_downcase", &[I], &[I]),
            Rt::StrReverse => ("grenat_str_reverse", &[I], &[I]),
            Rt::StrStrip => ("grenat_str_strip", &[I], &[I]),
            Rt::StrRepeat => ("grenat_str_repeat", &[I, I], &[I]),
            Rt::StrIncludes => ("grenat_str_includes", &[I, I], &[I]),
            Rt::StrStartsWith => ("grenat_str_starts_with", &[I, I], &[I]),
            Rt::StrEndsWith => ("grenat_str_ends_with", &[I, I], &[I]),
            Rt::StrToI => ("grenat_str_to_i", &[I], &[I]),
            Rt::StrToF => ("grenat_str_to_f", &[I], &[F]),
        }
    }
}

/// The runtime functions, declared in a module.
pub(crate) struct Runtime {
    ids: Vec<(Rt, FuncId)>,
}

/// The runtime functions, imported into one function.
pub(crate) type RuntimeRefs = HashMap<Rt, FuncRef>;

impl Runtime {
    pub fn declare(module: &mut impl Module) -> Result<Runtime, String> {
        let ids = Rt::ALL
            .iter()
            .map(|&rt| {
                let (symbol, params, returns) = rt.info();
                let mut sig = module.make_signature();
                sig.params.extend(params.iter().map(|t| AbiParam::new(*t)));
                sig.returns.extend(returns.iter().map(|t| AbiParam::new(*t)));
                let id = module.declare_function(symbol, Linkage::Import, &sig).map_err(|e| e.to_string())?;
                Ok((rt, id))
            })
            .collect::<Result<_, String>>()?;
        Ok(Runtime { ids })
    }

    pub fn import(&self, module: &mut impl Module, func: &mut Function) -> RuntimeRefs {
        self.ids.iter().map(|(rt, id)| (*rt, module.declare_func_in_func(*id, func))).collect()
    }
}

/// Every runtime symbol with its address, for the JIT's linker.
pub(crate) fn symbols() -> Vec<(&'static str, *const u8)> {
    grenat_runtime::symbols()
}
