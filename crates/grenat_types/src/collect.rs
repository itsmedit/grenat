//! Declaration collection: functions, types, messages, `include`, `@…` state.

use grenat_ast::{Field, Item, Member, Span, TypeKind};

use crate::ty::Ty;
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn collect(&mut self) {
        let program = self.program;
        for item in &program.items {
            match item {
                Item::Fn(def) => {
                    if self.fns.insert(&def.name.name, def).is_some() {
                        self.error(E_NAME, def.name.span, format!("function `{}` is defined twice", def.name.name));
                    }
                }
                Item::Type(def) => {
                    let decl = TypeDecl::new(def);
                    for (message, handler) in &decl.handlers {
                        self.messages.entry(message).or_default().push((&def.name.name, handler));
                    }
                    if self.types.insert(&def.name.name, decl).is_some() {
                        self.error(E_NAME, def.name.span, format!("type `{}` is defined twice", def.name.name));
                    }
                }
                Item::Model(model) => {
                    if self.models.contains(&model.name.name.as_str()) {
                        let message = format!("model `:{}` is declared twice (in config/models.yml and in code?)", model.name.name);
                        self.error(E_NAME, model.name.span, message);
                    }
                    self.models.push(&model.name.name);
                }
                // expanded before checking (see `grenat_macros`)
                Item::Stmt(_) | Item::Macro(_) => {}
            }
        }
        // `include Module`
        let includes: Vec<(&'p str, &'p str, Span)> = self
            .types
            .values()
            .flat_map(|t| {
                t.def.members.iter().filter_map(move |m| match m {
                    Member::Include(ty) => Some((t.def.name.name.as_str(), type_name(ty), ty.span())),
                    _ => None,
                })
            })
            .collect();
        for (owner, module, span) in includes {
            let Some(methods) =
                self.types.get(module).filter(|m| m.def.kind == TypeKind::Module).map(|m| m.methods.clone())
            else {
                self.error(E_NAME, span, format!("unknown module `{module}`"));
                continue;
            };
            let decl = self.types.get_mut(owner).expect("declared type");
            decl.includes.push(module);
            for (name, def) in methods {
                decl.methods.entry(name).or_insert(def);
            }
        }
        for decl in self.types.values() {
            for variant in &decl.variants {
                self.variants.insert(&variant.name.name, &decl.def.name.name);
            }
        }
    }

    /// `@writers = spawn_pool(Writer)`: `@writers` is a `Writer`.
    pub(crate) fn infer_ivars(&mut self) {
        let owners: Vec<(&'p str, Vec<&'p Field>)> =
            self.types.iter().map(|(name, decl)| (*name, decl.ivars.clone())).collect();
        for (owner, ivars) in owners {
            let mut cx = Ctx::new(Kind::Top, Some(Ty::user(owner)), None);
            for field in ivars {
                if let (None, Some(default)) = (&field.ty, &field.default) {
                    let v = self.expr(&mut cx, default);
                    self.ivar_types.insert((owner.to_string(), field.name.name.clone()), v.ty);
                }
            }
        }
    }
}
