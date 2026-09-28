//! A `native def` bound to the code that runs it: the declaration a program
//! holds must be the one `setter install` writes from what the facet
//! exports (its manifest) — the same parameters, result, effects and
//! purity — or the call is refused.
//!
//! The effects checked on the callers, the taint of the result and the
//! dangerous sinks all read the declaration: a declaration written by hand
//! (`native def read_text(path: String) -> String pure`) would otherwise
//! drop the effects and the taint of the code it calls.

use grenat_ast::{Effect, Param, Type};

use crate::prelude::*;

/// A `native def` as `grenat_native::declarations::signature` writes it:
/// `native def add(a: Int, b: Int) -> Int pure`.
pub(crate) fn declared(def: &FnDef) -> String {
    let mut out = format!("native def {}", def.name.name);
    if !def.params.is_empty() {
        let params: Vec<String> = def.params.iter().map(param).collect();
        out.push_str(&format!("({})", params.join(", ")));
    }
    if let Some(ret) = &def.ret {
        out.push_str(&format!(" -> {}", ty(ret)));
    }
    if !def.effects.is_empty() {
        let effects: Vec<String> = def.effects.iter().map(effect).collect();
        out.push_str(&format!(" uses {}", effects.join(", ")));
    }
    if def.pure {
        out.push_str(" pure");
    }
    out
}

fn param(p: &Param) -> String {
    let mut out = p.name.name.clone();
    if let Some(t) = &p.ty {
        out.push_str(&format!(": {}", ty(t)));
    }
    if p.default.is_some() {
        // never generated: a default value does not cross
        out.push_str(" = …");
    }
    out
}

fn ty(t: &Type) -> String {
    match t {
        Type::Named { path, args, .. } => {
            let name: Vec<&str> = path.iter().map(|i| i.name.as_str()).collect();
            let name = name.join("::");
            if args.is_empty() {
                name
            } else {
                format!("{name}({})", args.iter().map(ty).collect::<Vec<_>>().join(", "))
            }
        }
        Type::Optional(inner, _) => format!("{}?", ty(inner)),
        Type::Tainted(inner, _) => format!("~{}", ty(inner)),
    }
}

fn effect(e: &Effect) -> String {
    let path: Vec<&str> = e.path.iter().map(|i| i.name.as_str()).collect();
    let path = path.join(".");
    match e.args.as_slice() {
        [] => path,
        [arg] => match &arg.kind {
            ExprKind::Str(segments) => match segments.as_slice() {
                [StrSeg::Lit(text)] => format!("{path}(\"{text}\")"),
                _ => format!("{path}(…)"),
            },
            _ => format!("{path}(…)"),
        },
        _ => format!("{path}(…)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_def(src: &str) -> FnDef {
        let parsed = grenat_parser::parse(src);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        match parsed.program.items.into_iter().next() {
            Some(grenat_ast::Item::Fn(def)) => *def,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_declaration_is_written_as_setter_writes_it() {
        for line in [
            "native def add(a: Int, b: Int) -> Int pure",
            "native def read_sheet(path: String) -> ~Array(Array(String)) uses fs.read",
            "native def tick uses time, net(\"api.x.com\")",
            "native def first(values: Array(String?)) -> ~Hash(String, Int)?",
            "native def nothing",
        ] {
            assert_eq!(declared(&first_def(line)), line);
        }
        // what setter never writes cannot match a manifest
        assert_eq!(declared(&first_def("native def f(x: Int = 1)")), "native def f(x: Int = …)");
        assert_eq!(declared(&first_def("native def f uses net(host)")), "native def f uses net(…)");
    }
}
