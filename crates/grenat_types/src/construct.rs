//! Construction: structs, classes, variants, errors, agent messages.

use grenat_ast::{Diagnostic, Field, Span, TypeKind};

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// `Name(…)`: struct, variant, error, agent message, `Ok`/`Err`.
    pub(crate) fn construct(&mut self, span: Span, name: &str, argv: Vec<ArgV>) -> V {
        let taint = argv.iter().find_map(|a| a.v.taint);
        match name {
            "Ok" => {
                return V {
                    ty: Ty::Result(Box::new(argv.first().map_or(Ty::Nil, |a| a.v.ty.clone())), Box::new(Ty::Unknown)),
                    taint,
                };
            }
            "Err" => {
                return V {
                    ty: Ty::Result(
                        Box::new(Ty::Unknown),
                        Box::new(argv.first().map_or(Ty::Unknown, |a| a.v.ty.clone())),
                    ),
                    taint,
                };
            }
            _ => {}
        }
        if let Some(decl) = self.types.get(name) {
            let (kind, fields) = (decl.def.kind, decl.fields.clone());
            let init = decl.methods.get("initialize").copied();
            let ivars = decl.ivars.clone();
            match kind {
                TypeKind::Struct => {
                    self.bind_args(name, &Slot::fields(&fields), &argv, span);
                }
                TypeKind::Class => match init {
                    Some(def) => {
                        self.bind_args(name, &Slot::params(&def.params), &argv, span);
                    }
                    None => {
                        let slots: Vec<Slot<'p>> =
                            Slot::fields(&ivars).into_iter().map(|s| Slot { optional: true, ..s }).collect();
                        self.bind_args(name, &slots, &argv, span);
                    }
                },
                TypeKind::Agent => self.report(
                    Diagnostic::new(span, format!("`{name}` is an agent"))
                        .with_code(E_TYPE)
                        .with_help(format!("start it with `spawn {name}`")),
                ),
                _ => self.error(E_TYPE, span, format!("`{name}` cannot be instantiated")),
            }
            return V { ty: Ty::user(name), taint };
        }
        if let Some(enum_name) = self.variants.get(name).copied() {
            let variant =
                self.types[enum_name].variants.iter().find(|v| v.name.name == name).copied().expect("variant");
            let fields: Vec<&'p Field> = variant.fields.iter().collect();
            self.bind_args(name, &Slot::fields(&fields), &argv, span);
            return V { ty: Ty::user(enum_name), taint };
        }
        if is_error_name(name) {
            return V { ty: Ty::user(name), taint };
        }
        if let Some(handlers) = self.messages.get(name).cloned() {
            let (_, handler) = handlers[0];
            self.bind_args(name, &Slot::params(&handler.params), &argv, span);
            return V { ty: Ty::user(name), taint };
        }
        let known: Vec<&str> =
            self.types.keys().chain(self.variants.keys()).chain(self.messages.keys()).copied().collect();
        self.error_help(E_NAME, span, format!("unknown type `{name}`"), suggest(name, known));
        V { ty: Ty::Unknown, taint }
    }
}
