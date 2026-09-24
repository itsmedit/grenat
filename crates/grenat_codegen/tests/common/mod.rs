//! Helpers shared by the code generation tests.
#![allow(dead_code)]

use grenat_ast::{FnDef, Item, Program};
use grenat_codegen::{Jit, Scalar, Trap};

/// Parses `src` (leaked: the JIT is keyed by the program's AST) and compiles it.
pub fn compile(src: &str) -> (&'static Program, Jit) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let program: &'static Program = Box::leak(Box::new(parsed.program));
    let jit = Jit::compile(program).expect("compilation");
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

/// Calls a compiled function; panics if it was not compiled.
pub fn call(program: &Program, jit: &Jit, name: &str, args: &[Scalar]) -> Result<Scalar, Trap> {
    jit.call(function(program, name), args, 10_000).unwrap_or_else(|| panic!("`{name}` is not compiled"))
}

pub fn int(n: i64) -> Scalar {
    Scalar::Int(n)
}

pub fn float(f: f64) -> Scalar {
    Scalar::Float(f)
}
