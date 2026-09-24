//! Which functions are compiled to native code.
//!
//! A *candidate* is a top-level `def` whose parameters and return type are all
//! annotated `Int`, `Float` or `Bool`, with no effects and no default values.
//! A candidate is compiled if its body types (see [`infer`](crate::infer))
//! against the signatures of the other compiled functions; removing one
//! candidate can disqualify its callers, hence the fixed point.

use grenat_ast::{FnDef, FnKind, Item, Program, Type};

use crate::infer::{Signature, Signatures, Typed, infer};
use crate::scalar::ScalarTy;

pub(crate) struct Compiled<'p> {
    pub def: &'p FnDef,
    pub sig: Signature,
    pub typed: Typed,
}

/// Functions to compile, and the candidates left to the interpreter with the reason.
pub(crate) fn select(program: &Program) -> (Vec<Compiled<'_>>, Vec<(String, String)>) {
    let mut candidates: Vec<(&FnDef, Signature)> = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(def) => signature(def).map(|sig| (&**def, sig)),
            _ => None,
        })
        .collect();
    let mut rejected = Vec::new();
    loop {
        let sigs: Signatures = candidates.iter().map(|(def, sig)| (def.name.name.as_str(), sig.clone())).collect();
        let mut compiled = Vec::new();
        let mut changed = false;
        for (def, sig) in std::mem::take(&mut candidates) {
            match infer(def, &sig, &sigs) {
                Ok(typed) => compiled.push(Compiled { def, sig, typed }),
                Err(reason) => {
                    rejected.push((def.name.name.clone(), reason));
                    changed = true;
                }
            }
        }
        if !changed {
            return (compiled, rejected);
        }
        candidates = compiled.into_iter().map(|c| (c.def, c.sig)).collect();
    }
}

/// Native signature of a candidate, or `None` for an ordinary function.
fn signature(def: &FnDef) -> Option<Signature> {
    let plain = def.kind == FnKind::Def && !def.is_abstract && !def.on_self && def.effects.is_empty();
    if !plain {
        return None;
    }
    let params = def
        .params
        .iter()
        .map(|p| if p.default.is_some() { None } else { p.ty.as_ref().and_then(scalar) })
        .collect::<Option<Vec<_>>>()?;
    let ret = def.ret.as_ref().and_then(scalar)?;
    Some(Signature { params, ret })
}

fn scalar(ty: &Type) -> Option<ScalarTy> {
    match ty {
        Type::Named { path, args, .. } if args.is_empty() && path.len() == 1 => ScalarTy::from_name(&path[0].name),
        _ => None,
    }
}
