//! A Cranelift function in LLVM IR.
//!
//! Values keep their Cranelift numbers (`%v12`), blocks too (`b3`); block
//! parameters become `phi`s, fed by every branch to the block. A `brif`
//! goes through two edge blocks, so that each incoming value comes from a
//! distinct predecessor. Addresses are `i64`, turned into pointers where
//! memory is accessed; flags are `i8`, as in Cranelift.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    Block, BlockArg, Function, GlobalValueData, InstructionData, Opcode, Signature, Type, Value, types,
};

use super::names::Names;

/// `RET @name(PARAMS)`, with parameter names when `params` are given.
pub(crate) fn prototype(name: &str, sig: &Signature, params: Option<&[String]>) -> Result<String, String> {
    let mut list = Vec::new();
    for (i, param) in sig.params.iter().enumerate() {
        let ty = ty(param.value_type)?;
        list.push(match params {
            Some(names) => format!("{ty} {}", names[i]),
            None => ty.to_string(),
        });
    }
    Ok(format!("{} {name}({})", ret_type(sig)?, list.join(", ")))
}

fn ret_type(sig: &Signature) -> Result<String, String> {
    let returns: Vec<&str> = sig.returns.iter().map(|r| ty(r.value_type)).collect::<Result<_, _>>()?;
    Ok(match returns.as_slice() {
        [] => "void".into(),
        [one] => one.to_string(),
        many => format!("{{ {} }}", many.join(", ")),
    })
}

pub(crate) fn ty(t: Type) -> Result<&'static str, String> {
    Ok(match t {
        types::I8 => "i8",
        types::I16 => "i16",
        types::I32 => "i32",
        types::I64 => "i64",
        types::F32 => "float",
        types::F64 => "double",
        other => return Err(format!("unsupported type `{other}`")),
    })
}

/// The definition of `func`, named `name`; the intrinsics it uses are added
/// to `intrinsics` (as declarations).
pub(crate) fn define(
    names: &Names,
    name: &str,
    linkage: &str,
    func: &Function,
    intrinsics: &mut BTreeSet<String>,
) -> Result<String, String> {
    let mut t = Translation { names, func, intrinsics, fresh: 0, incoming: HashMap::new() };
    let entry = func.layout.entry_block().ok_or("a function without blocks")?;
    let params: Vec<String> = (0..func.signature.params.len()).map(|i| format!("%arg{i}")).collect();
    let args: Vec<String> = params.clone();
    t.incoming.entry(entry).or_default().push(("entry".into(), args));

    let mut bodies = Vec::new();
    for block in func.layout.blocks() {
        bodies.push((block, t.block(block)?));
    }
    let mut out = format!("define {linkage}{} #0 {{\nentry:\n  br label %b{}\n", prototype(name, &func.signature, Some(&params))?, entry.as_u32());
    for (block, body) in bodies {
        writeln!(out, "b{}:", block.as_u32()).unwrap();
        let incoming = t.incoming.get(&block).cloned().unwrap_or_default();
        for (i, param) in func.dfg.block_params(block).iter().enumerate() {
            let ty = ty(func.dfg.value_type(*param))?;
            let pairs: Vec<String> = incoming.iter().map(|(label, args)| format!("[ {}, %{label} ]", args[i])).collect();
            writeln!(out, "  {} = phi {ty} {}", value(*param), pairs.join(", ")).unwrap();
        }
        out.push_str(&body);
    }
    out.push_str("}\n");
    Ok(out)
}

struct Translation<'a> {
    names: &'a Names<'a>,
    func: &'a Function,
    intrinsics: &'a mut BTreeSet<String>,
    /// Numbers temporaries and extra labels.
    fresh: usize,
    /// For each block: (predecessor label, the arguments it passes).
    incoming: HashMap<Block, Vec<(String, Vec<String>)>>,
}

fn value(v: Value) -> String {
    format!("%v{}", v.as_u32())
}

impl Translation<'_> {
    fn temp(&mut self) -> String {
        self.fresh += 1;
        format!("%t{}", self.fresh)
    }

    fn label(&mut self, what: &str) -> String {
        self.fresh += 1;
        format!("{what}{}", self.fresh)
    }

    fn arg(&self, v: Value) -> String {
        value(self.func.dfg.resolve_aliases(v))
    }

    fn vty(&self, v: Value) -> Result<&'static str, String> {
        ty(self.func.dfg.value_type(v))
    }

    fn intrinsic(&mut self, declaration: &str) {
        self.intrinsics.insert(format!("declare {declaration}"));
    }

    /// The instructions of `block` (without its phis).
    fn block(&mut self, block: Block) -> Result<String, String> {
        let mut out = String::new();
        let mut label = format!("b{}", block.as_u32());
        for inst in self.func.layout.block_insts(block) {
            let data = &self.func.dfg.insts[inst];
            let args: Vec<Value> = self.func.dfg.inst_args(inst).to_vec();
            let results: Vec<Value> = self.func.dfg.inst_results(inst).to_vec();
            let operands: Vec<String> = args.iter().map(|v| self.arg(*v)).collect();
            let a = |i: usize| operands[i].clone();
            let r = |i: usize| value(results[i]);
            let opcode = data.opcode();
            match (opcode, data) {
                (Opcode::Iconst, InstructionData::UnaryImm { imm, .. }) => {
                    let ty = self.vty(results[0])?;
                    let bits = imm.bits();
                    let constant = match ty {
                        "i8" => (bits as i8).to_string(),
                        "i16" => (bits as i16).to_string(),
                        "i32" => (bits as i32).to_string(),
                        _ => bits.to_string(),
                    };
                    writeln!(out, "  {} = add {ty} 0, {constant}", r(0)).unwrap();
                }
                (Opcode::F64const, InstructionData::UnaryIeee64 { imm, .. }) => {
                    writeln!(out, "  {} = bitcast i64 {} to double", r(0), imm.bits() as i64).unwrap();
                }
                (
                    Opcode::Iadd | Opcode::Isub | Opcode::Imul | Opcode::Sdiv | Opcode::Srem | Opcode::Udiv | Opcode::Urem
                    | Opcode::Band | Opcode::Bor | Opcode::Bxor | Opcode::Fadd | Opcode::Fsub | Opcode::Fmul | Opcode::Fdiv,
                    _,
                ) => {
                    let op = match opcode {
                        Opcode::Iadd => "add",
                        Opcode::Isub => "sub",
                        Opcode::Imul => "mul",
                        Opcode::Sdiv => "sdiv",
                        Opcode::Srem => "srem",
                        Opcode::Udiv => "udiv",
                        Opcode::Urem => "urem",
                        Opcode::Band => "and",
                        Opcode::Bor => "or",
                        Opcode::Bxor => "xor",
                        Opcode::Fadd => "fadd",
                        Opcode::Fsub => "fsub",
                        Opcode::Fmul => "fmul",
                        _ => "fdiv",
                    };
                    writeln!(out, "  {} = {op} {} {}, {}", r(0), self.vty(args[0])?, a(0), a(1)).unwrap();
                }
                (Opcode::Ishl | Opcode::Sshr | Opcode::Ushr, _) => {
                    // Cranelift takes the shift amount modulo the width
                    let ty = self.vty(args[0])?;
                    let width = self.func.dfg.value_type(args[0]).bits();
                    let amount = self.temp();
                    let cast = self.temp();
                    let aty = self.vty(args[1])?;
                    writeln!(out, "  {amount} = and {aty} {}, {}", a(1), width - 1).unwrap();
                    let conv = if aty == ty { "bitcast" } else if self.func.dfg.value_type(args[1]).bits() > width { "trunc" } else { "zext" };
                    if conv == "bitcast" {
                        writeln!(out, "  {cast} = add {ty} {amount}, 0").unwrap();
                    } else {
                        writeln!(out, "  {cast} = {conv} {aty} {amount} to {ty}").unwrap();
                    }
                    let op = match opcode {
                        Opcode::Ishl => "shl",
                        Opcode::Sshr => "ashr",
                        _ => "lshr",
                    };
                    writeln!(out, "  {} = {op} {ty} {}, {cast}", r(0), a(0)).unwrap();
                }
                (Opcode::Ineg, _) => writeln!(out, "  {} = sub {} 0, {}", r(0), self.vty(args[0])?, a(0)).unwrap(),
                (Opcode::Iabs, _) => {
                    let ty = self.vty(args[0])?;
                    self.intrinsic(&format!("{ty} @llvm.abs.{ty}({ty}, i1)"));
                    writeln!(out, "  {} = call {ty} @llvm.abs.{ty}({ty} {}, i1 false)", r(0), a(0)).unwrap();
                }
                (Opcode::Fneg, _) => writeln!(out, "  {} = fneg {} {}", r(0), self.vty(args[0])?, a(0)).unwrap(),
                (Opcode::Fabs | Opcode::Sqrt | Opcode::Floor | Opcode::Ceil | Opcode::Trunc | Opcode::Nearest, _) => {
                    let ty = self.vty(args[0])?;
                    let suffix = if ty == "double" { "f64" } else { "f32" };
                    let name = match opcode {
                        Opcode::Fabs => "fabs",
                        Opcode::Sqrt => "sqrt",
                        Opcode::Floor => "floor",
                        Opcode::Ceil => "ceil",
                        Opcode::Trunc => "trunc",
                        _ => "roundeven",
                    };
                    self.intrinsic(&format!("{ty} @llvm.{name}.{suffix}({ty})"));
                    writeln!(out, "  {} = call {ty} @llvm.{name}.{suffix}({ty} {})", r(0), a(0)).unwrap();
                }
                (Opcode::SaddOverflow | Opcode::SsubOverflow | Opcode::SmulOverflow, _) => {
                    let ty = self.vty(args[0])?;
                    let name = match opcode {
                        Opcode::SaddOverflow => "sadd",
                        Opcode::SsubOverflow => "ssub",
                        _ => "smul",
                    };
                    let intrinsic = format!("@llvm.{name}.with.overflow.{ty}");
                    self.intrinsic(&format!("{{ {ty}, i1 }} {intrinsic}({ty}, {ty})"));
                    let (pair, flag) = (self.temp(), self.temp());
                    writeln!(out, "  {pair} = call {{ {ty}, i1 }} {intrinsic}({ty} {}, {ty} {})", a(0), a(1)).unwrap();
                    writeln!(out, "  {} = extractvalue {{ {ty}, i1 }} {pair}, 0", r(0)).unwrap();
                    writeln!(out, "  {flag} = extractvalue {{ {ty}, i1 }} {pair}, 1").unwrap();
                    writeln!(out, "  {} = zext i1 {flag} to {}", r(1), self.vty(results[1])?).unwrap();
                }
                (Opcode::Icmp, InstructionData::IntCompare { cond, .. }) => {
                    let flag = self.temp();
                    writeln!(out, "  {flag} = icmp {} {} {}, {}", int_cc(*cond), self.vty(args[0])?, a(0), a(1)).unwrap();
                    writeln!(out, "  {} = zext i1 {flag} to {}", r(0), self.vty(results[0])?).unwrap();
                }
                (Opcode::Fcmp, InstructionData::FloatCompare { cond, .. }) => {
                    let flag = self.temp();
                    writeln!(out, "  {flag} = fcmp {} {} {}, {}", float_cc(*cond), self.vty(args[0])?, a(0), a(1)).unwrap();
                    writeln!(out, "  {} = zext i1 {flag} to {}", r(0), self.vty(results[0])?).unwrap();
                }
                (Opcode::Select, _) => {
                    let flag = self.temp();
                    writeln!(out, "  {flag} = icmp ne {} {}, 0", self.vty(args[0])?, a(0)).unwrap();
                    let ty = self.vty(args[1])?;
                    writeln!(out, "  {} = select i1 {flag}, {ty} {}, {ty} {}", r(0), a(1), a(2)).unwrap();
                }
                (Opcode::Bitcast, _) => {
                    writeln!(out, "  {} = bitcast {} {} to {}", r(0), self.vty(args[0])?, a(0), self.vty(results[0])?).unwrap();
                }
                (Opcode::Uextend | Opcode::Sextend | Opcode::Ireduce | Opcode::FcvtFromSint | Opcode::FcvtFromUint, _) => {
                    let op = match opcode {
                        Opcode::Uextend => "zext",
                        Opcode::Sextend => "sext",
                        Opcode::Ireduce => "trunc",
                        Opcode::FcvtFromSint => "sitofp",
                        _ => "uitofp",
                    };
                    writeln!(out, "  {} = {op} {} {} to {}", r(0), self.vty(args[0])?, a(0), self.vty(results[0])?).unwrap();
                }
                (Opcode::FcvtToSintSat | Opcode::FcvtToUintSat, _) => {
                    let (from, to) = (self.vty(args[0])?, self.vty(results[0])?);
                    let sign = if opcode == Opcode::FcvtToSintSat { "fptosi" } else { "fptoui" };
                    let suffix = if from == "double" { "f64" } else { "f32" };
                    let intrinsic = format!("@llvm.{sign}.sat.{to}.{suffix}");
                    self.intrinsic(&format!("{to} {intrinsic}({from})"));
                    writeln!(out, "  {} = call {to} {intrinsic}({from} {})", r(0), a(0)).unwrap();
                }
                (Opcode::Load, InstructionData::Load { offset, .. }) => {
                    let pointer = self.address(&mut out, &a(0), i64::from(*offset));
                    let ty = self.vty(results[0])?;
                    writeln!(out, "  {} = load {ty}, ptr {pointer}, align {}", r(0), self.func.dfg.value_type(results[0]).bytes()).unwrap();
                }
                (Opcode::Store, InstructionData::Store { offset, .. }) => {
                    let pointer = self.address(&mut out, &a(1), i64::from(*offset));
                    let ty = self.vty(args[0])?;
                    writeln!(out, "  store {ty} {}, ptr {pointer}, align {}", a(0), self.func.dfg.value_type(args[0]).bytes()).unwrap();
                }
                (Opcode::SymbolValue, InstructionData::UnaryGlobalValue { global_value, .. }) => {
                    let GlobalValueData::Symbol { name, offset, .. } = &self.func.global_values[*global_value] else {
                        return Err("unsupported global value".into());
                    };
                    let symbol = self.names.external(self.func, name)?;
                    let base = self.temp();
                    writeln!(out, "  {base} = ptrtoint ptr {symbol} to i64").unwrap();
                    writeln!(out, "  {} = add i64 {base}, {}", r(0), offset.bits()).unwrap();
                }
                (Opcode::Call, InstructionData::Call { func_ref, .. }) => {
                    let ext = &self.func.dfg.ext_funcs[*func_ref];
                    let sig = &self.func.dfg.signatures[ext.signature];
                    let callee = self.names.external(self.func, &ext.name)?;
                    let list: Vec<String> =
                        args.iter().map(|v| Ok(format!("{} {}", self.vty(*v)?, self.arg(*v)))).collect::<Result<_, String>>()?;
                    let ret = ret_type(sig)?;
                    match results.len() {
                        0 => writeln!(out, "  call {ret} {callee}({})", list.join(", ")).unwrap(),
                        1 => writeln!(out, "  {} = call {ret} {callee}({})", r(0), list.join(", ")).unwrap(),
                        _ => {
                            let pair = self.temp();
                            writeln!(out, "  {pair} = call {ret} {callee}({})", list.join(", ")).unwrap();
                            for i in 0..results.len() {
                                writeln!(out, "  {} = extractvalue {ret} {pair}, {i}", r(i)).unwrap();
                            }
                        }
                    }
                }
                (Opcode::Jump, InstructionData::Jump { destination, .. }) => {
                    let (target, passed) = self.edge(destination)?;
                    self.incoming.entry(target).or_default().push((label.clone(), passed));
                    writeln!(out, "  br label %b{}", target.as_u32()).unwrap();
                }
                (Opcode::Brif, InstructionData::Brif { blocks, .. }) => {
                    let flag = self.temp();
                    writeln!(out, "  {flag} = icmp ne {} {}, 0", self.vty(args[0])?, a(0)).unwrap();
                    let (then_label, else_label) = (self.label("then"), self.label("else"));
                    writeln!(out, "  br i1 {flag}, label %{then_label}, label %{else_label}").unwrap();
                    for (edge, call) in [(then_label, &blocks[0]), (else_label, &blocks[1])] {
                        let (target, passed) = self.edge(call)?;
                        self.incoming.entry(target).or_default().push((edge.clone(), passed));
                        writeln!(out, "{edge}:\n  br label %b{}", target.as_u32()).unwrap();
                    }
                }
                (Opcode::Return, _) => match args.len() {
                    0 => out.push_str("  ret void\n"),
                    1 => writeln!(out, "  ret {} {}", self.vty(args[0])?, a(0)).unwrap(),
                    _ => {
                        let ret = ret_type(&self.func.signature)?;
                        let mut aggregate = "undef".to_string();
                        for (i, v) in args.iter().enumerate() {
                            let next = self.temp();
                            writeln!(out, "  {next} = insertvalue {ret} {aggregate}, {} {}, {i}", self.vty(*v)?, self.arg(*v)).unwrap();
                            aggregate = next;
                        }
                        writeln!(out, "  ret {ret} {aggregate}").unwrap();
                    }
                },
                (Opcode::Trap, _) => {
                    self.intrinsic("void @llvm.trap()");
                    out.push_str("  call void @llvm.trap()\n  unreachable\n");
                }
                (Opcode::Trapz | Opcode::Trapnz, _) => {
                    self.intrinsic("void @llvm.trap()");
                    let flag = self.temp();
                    let cc = if opcode == Opcode::Trapz { "eq" } else { "ne" };
                    writeln!(out, "  {flag} = icmp {cc} {} {}, 0", self.vty(args[0])?, a(0)).unwrap();
                    let (trap, next) = (self.label("trap"), self.label("continue"));
                    writeln!(out, "  br i1 {flag}, label %{trap}, label %{next}").unwrap();
                    writeln!(out, "{trap}:\n  call void @llvm.trap()\n  unreachable\n{next}:").unwrap();
                    label = next;
                }
                (Opcode::Nop, _) => {}
                (other, _) => return Err(format!("unsupported instruction `{other}`")),
            }
        }
        Ok(out)
    }

    /// `base + offset` as a pointer.
    fn address(&mut self, out: &mut String, base: &str, offset: i64) -> String {
        let sum = self.temp();
        let pointer = self.temp();
        writeln!(out, "  {sum} = add i64 {base}, {offset}").unwrap();
        writeln!(out, "  {pointer} = inttoptr i64 {sum} to ptr").unwrap();
        pointer
    }

    /// The block a branch goes to, and the values it passes.
    fn edge(&self, call: &cranelift_codegen::ir::BlockCall) -> Result<(Block, Vec<String>), String> {
        let pool = &self.func.dfg.value_lists;
        let passed = call
            .args(pool)
            .map(|arg| match arg {
                BlockArg::Value(v) => Ok(self.arg(v)),
                _ => Err("unsupported block argument".to_string()),
            })
            .collect::<Result<_, _>>()?;
        Ok((call.block(pool), passed))
    }
}

fn int_cc(cc: IntCC) -> &'static str {
    match cc {
        IntCC::Equal => "eq",
        IntCC::NotEqual => "ne",
        IntCC::SignedLessThan => "slt",
        IntCC::SignedGreaterThanOrEqual => "sge",
        IntCC::SignedGreaterThan => "sgt",
        IntCC::SignedLessThanOrEqual => "sle",
        IntCC::UnsignedLessThan => "ult",
        IntCC::UnsignedGreaterThanOrEqual => "uge",
        IntCC::UnsignedGreaterThan => "ugt",
        IntCC::UnsignedLessThanOrEqual => "ule",
    }
}

fn float_cc(cc: FloatCC) -> &'static str {
    match cc {
        FloatCC::Ordered => "ord",
        FloatCC::Unordered => "uno",
        FloatCC::Equal => "oeq",
        FloatCC::NotEqual => "une",
        FloatCC::OrderedNotEqual => "one",
        FloatCC::UnorderedOrEqual => "ueq",
        FloatCC::LessThan => "olt",
        FloatCC::LessThanOrEqual => "ole",
        FloatCC::GreaterThan => "ogt",
        FloatCC::GreaterThanOrEqual => "oge",
        FloatCC::UnorderedOrLessThan => "ult",
        FloatCC::UnorderedOrLessThanOrEqual => "ule",
        FloatCC::UnorderedOrGreaterThan => "ugt",
        FloatCC::UnorderedOrGreaterThanOrEqual => "uge",
    }
}
