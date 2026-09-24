//! Which functions are compiled to native code.
//!
//! A *candidate* is a top-level `def` whose parameters and return type all
//! have native types (`Int`, `Float`, `Bool`, `String`, `Array(T)`, native
//! structs), with no effects and no default values.
//! A candidate is compiled if its body types (see [`infer`](crate::infer))
//! against the signatures of the other compiled functions; removing one
//! candidate can disqualify its callers, hence the fixed point.

use std::collections::HashSet;

use grenat_ast::{FnDef, FnKind, Item, Program};

use crate::infer::{Signature, Signatures, Typed, infer};
use crate::structs::Structs;

pub(crate) struct Compiled<'p> {
    pub def: &'p FnDef,
    pub sig: Signature,
    pub typed: Typed,
    /// May modify an array, itself or through the functions it calls.
    pub mutates: bool,
}

/// Functions to compile, and the candidates left to the interpreter with the reason.
pub(crate) fn select<'p>(program: &'p Program, structs: &Structs) -> (Vec<Compiled<'p>>, Vec<(String, String)>) {
    let mut candidates: Vec<(&FnDef, Signature)> = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(def) => signature(def, structs).map(|sig| (&**def, sig)),
            _ => None,
        })
        .collect();
    let mut rejected = Vec::new();
    loop {
        let sigs: Signatures = candidates.iter().map(|(def, sig)| (def.name.name.as_str(), sig.clone())).collect();
        let mut compiled = Vec::new();
        let mut changed = false;
        for (def, sig) in std::mem::take(&mut candidates) {
            match infer(def, &sig, &sigs, structs) {
                Ok(typed) => compiled.push(Compiled { def, sig, typed, mutates: false }),
                Err(reason) => {
                    rejected.push((def.name.name.clone(), reason));
                    changed = true;
                }
            }
        }
        if !changed {
            mutations(&mut compiled);
            return (compiled, rejected);
        }
        candidates = compiled.into_iter().map(|c| (c.def, c.sig)).collect();
    }
}

/// Which functions may modify an array, directly or through a callee.
fn mutations(compiled: &mut [Compiled]) {
    let mut mutating: HashSet<String> =
        compiled.iter().filter(|c| c.typed.mutates).map(|c| c.def.name.name.clone()).collect();
    loop {
        let more: Vec<String> = compiled
            .iter()
            .filter(|c| !mutating.contains(&c.def.name.name) && c.typed.calls.iter().any(|f| mutating.contains(f)))
            .map(|c| c.def.name.name.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        mutating.extend(more);
    }
    for c in compiled {
        c.mutates = mutating.contains(&c.def.name.name);
    }
}

/// Native signature of a candidate, or `None` for an ordinary function.
fn signature(def: &FnDef, structs: &Structs) -> Option<Signature> {
    let plain = def.kind == FnKind::Def && !def.is_abstract && !def.on_self && def.effects.is_empty();
    if !plain {
        return None;
    }
    let params = def
        .params
        .iter()
        .map(|p| if p.default.is_some() { None } else { structs.ty(p.ty.as_ref()?) })
        .collect::<Option<Vec<_>>>()?;
    let ret = structs.ty(def.ret.as_ref()?)?;
    Some(Signature { params, ret })
}
