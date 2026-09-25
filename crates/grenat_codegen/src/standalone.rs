//! Whole programs without the interpreter (`grenat build --native`).
//!
//! Every function of the program, `main` included, is compiled; the object
//! exports a `grenat_runtime::abi::Standalone` (`main`'s entry point, the
//! source, the error sites) and is linked with the small `grenat_standalone`
//! library, which runs `main` and reports errors. The program is only
//! machine code and the runtime: no parser, no interpreter.
//!
//! What native code cannot represent (`nil`, where the interpreter would go
//! on) is an error here, reported where it happens.

use cranelift_module::{Linkage, Module};
use grenat_ast::{FnKind, Item, Program, TypeKind};
use grenat_runtime::abi::STANDALONE_SYMBOL;

use crate::aot::Object;
use crate::eligibility::select;
use crate::Backend;
use crate::emit::emit;
use crate::infer::Target;
use crate::native::Report;
use crate::object_data::{Strings, Word, new_record, record};
use crate::structs::Structs;
use crate::ty::{Elem, Ty};

/// The object file of a standalone `program` (parsed from `source`, whose
/// file table is `files`), or why the program needs the interpreter.
pub fn object(program: &Program, source: &str, files: &str, backend: Backend) -> Result<Object, Vec<String>> {
    let structs = Structs::from_program(program);
    let (selected, rejected) = select(program, &structs, Target::Standalone);
    let mut reasons = interpreter_only(program, &selected.iter().map(|c| c.def.name.name.as_str()).collect::<Vec<_>>());
    reasons.extend(rejected.iter().map(|(name, reason)| format!("`{name}` cannot be compiled: it {reason}")));
    let main = selected.iter().position(|c| c.def.name.name == "main");
    let takes_args = match main.map(|i| selected[i].sig.params.as_slice()) {
        None if reasons.is_empty() => {
            reasons.push("there is no compiled `main` to run".into());
            false
        }
        None => false,
        Some([]) => false,
        Some([Ty::Array(Elem::Str)]) => true,
        Some(_) => {
            reasons.push("`main` must take no parameter, or `args: Array(String)`".into());
            false
        }
    };
    if !reasons.is_empty() {
        return Err(reasons);
    }
    let main = main.expect("checked");

    let (bytes, ()) = backend
        .object(|mut module| descriptor(&mut module, &selected, &structs, main, takes_args, source, files))
        .map_err(|e| vec![e])?;
    let report = Report { compiled: selected.iter().map(|c| c.def.name.name.clone()).collect(), interpreted: rejected };
    Ok(Object { bytes, report })
}

/// The compiled functions and the [`Standalone`](grenat_runtime::abi::Standalone) describing them.
fn descriptor(
    module: &mut impl Module,
    selected: &[crate::eligibility::Compiled],
    structs: &Structs,
    main: usize,
    takes_args: bool,
    source: &str,
    files: &str,
) -> Result<(), String> {
    let fail = |e: cranelift_module::ModuleError| e.to_string();
    let emitted = emit(module, selected, structs)?;
    let mut strings = Strings::default();
    let one = |e: String| e;

    let mut sites = Vec::new();
    for site in &emitted.sites {
        let [function_ptr, function_len] = strings.words(module, &site.function).map_err(one)?;
        let [reason_ptr, reason_len] = strings.words(module, site.reason.unwrap_or("")).map_err(one)?;
        sites.extend([
            function_ptr,
            function_len,
            Word::Number(u64::from(site.function_span.start)),
            Word::Number(u64::from(site.function_span.end)),
            Word::Number(u64::from(site.span.start)),
            Word::Number(u64::from(site.span.end)),
            reason_ptr,
            reason_len,
        ]);
    }
    let sites_table = new_record(module, &sites).map_err(one)?;
    let [source_ptr, source_len] = strings.words(module, source).map_err(one)?;
    let [path_ptr, path_len] = strings.words(module, files).map_err(one)?;
    let descriptor = module.declare_data(STANDALONE_SYMBOL, Linkage::Export, false, false).map_err(fail)?;
    let words = [
        Word::Function(emitted.trampolines[main]),
        Word::Number(u64::from(takes_args)),
        source_ptr,
        source_len,
        path_ptr,
        path_len,
        Word::Data(sites_table),
        Word::Number(emitted.sites.len() as u64),
    ];
    record(module, descriptor, &words)
}

/// Parts of the program only the interpreter runs.
fn interpreter_only(program: &Program, compiled: &[&str]) -> Vec<String> {
    let mut reasons = Vec::new();
    for item in &program.items {
        match item {
            Item::Stmt(_) => {
                reasons.push("top-level statements need the interpreter: move them into `main`".into());
            }
            Item::Fn(def) if def.kind != FnKind::Def => {
                reasons.push(format!("`{}` is a {:?}, which needs the interpreter", def.name.name, def.kind).to_lowercase());
            }
            Item::Fn(def) if !compiled.contains(&def.name.name.as_str()) && def.params.iter().any(|p| p.ty.is_none()) => {
                reasons.push(format!("`{}` cannot be compiled: its parameters need types", def.name.name));
            }
            Item::Type(def) if matches!(def.kind, TypeKind::Agent | TypeKind::Supervisor) => {
                reasons.push(format!("`{}` is an agent, which needs the interpreter", def.name.name));
            }
            Item::Model(decl) => {
                reasons.push(format!("model `:{}` needs the interpreter", decl.name.name));
            }
            _ => {}
        }
    }
    reasons.dedup();
    reasons
}
