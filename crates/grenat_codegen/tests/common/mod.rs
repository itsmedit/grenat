//! Helpers shared by the code generation tests.
#![allow(dead_code)]

use grenat_ast::{FnDef, Item, Program};
use grenat_codegen::{Data, Failure, Native, Returned, Trap};
use grenat_runtime::live_objects;

/// Parses `src` (leaked: the JIT is keyed by the program's AST) and compiles it.
pub fn compile(src: &str) -> (&'static Program, Native) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let program: &'static Program = Box::leak(Box::new(parsed.program));
    let jit = Native::compile(program).expect("compilation");
    (program, jit)
}

pub fn function<'p>(program: &'p Program, name: &str) -> &'p FnDef {
    program
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(def) if def.name.name == name => Some(&**def),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no function `{name}`"))
}

/// Calls a compiled function; panics if it was not compiled, and if the call
/// leaves any object alive (every call must free what it allocates, errors included).
pub fn call_full(program: &Program, jit: &Native, name: &str, args: &[Data]) -> Result<Returned, Failure> {
    let before = live_objects();
    let result = jit
        .call(function(program, name), args, 10_000, &|| false)
        .unwrap_or_else(|| panic!("`{name}` is not compiled"));
    assert_eq!(live_objects(), before, "`{name}` leaks objects");
    result
}

/// The value of a call that neither deoptimizes nor needs its arrays back.
pub fn call(program: &Program, jit: &Native, name: &str, args: &[Data]) -> Result<Data, Trap> {
    match call_full(program, jit, name, args) {
        Ok(returned) => Ok(returned.value),
        Err(Failure::Trap(trap)) => Err(trap),
        Err(Failure::Deopt) => panic!("`{name}` deoptimized"),
    }
}

pub fn int(n: i64) -> Data {
    Data::Int(n)
}

pub fn float(f: f64) -> Data {
    Data::Float(f)
}

pub fn boolean(b: bool) -> Data {
    Data::Bool(b)
}

pub fn string(s: &str) -> Data {
    Data::Str(s.into())
}

pub fn ints(items: &[i64]) -> Data {
    Data::Array(items.iter().map(|n| Data::Int(*n)).collect())
}

pub fn strings(items: &[&str]) -> Data {
    Data::Array(items.iter().map(|s| string(s)).collect())
}
