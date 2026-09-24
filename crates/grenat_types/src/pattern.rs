//! Motifs de `case … in`.

use grenat_ast::{Pattern, PatternKind, TypeKind};

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn pattern(&mut self, cx: &mut Ctx<'p>, pattern: &'p Pattern, subject: &V) {
        match &pattern.kind {
            PatternKind::Wildcard => {}
            PatternKind::Bind(name) => cx.define(name, subject.clone()),
            PatternKind::Lit(e) => {
                self.expr(cx, e);
            }
            PatternKind::Const { path, fields } => {
                let name = path.last().expect("chemin").name.as_str();
                let slots: Option<Vec<Slot<'p>>> = if let Some(enum_name) = self.variants.get(name).copied() {
                    if let Ty::User(subject_enum) = subject.ty.base()
                        && self.types.get(subject_enum.as_str()).is_some_and(|t| t.def.kind == TypeKind::Enum)
                        && subject_enum != enum_name
                    {
                        self.error(
                            E_TYPE,
                            pattern.span,
                            format!("`{name}` est une variante de `{enum_name}`, pas de `{subject_enum}`"),
                        );
                    }
                    let variant =
                        self.types[enum_name].variants.iter().find(|v| v.name.name == name).copied().expect("variante");
                    Some(Slot::fields(&variant.fields.iter().collect::<Vec<_>>()))
                } else if let Some(decl) = self.types.get(name) {
                    Some(Slot::fields(&decl.fields))
                } else if is_error_name(name) || builtins::TYPE_NAMES.contains(&name) {
                    None
                } else {
                    let known: Vec<&str> = self.variants.keys().chain(self.types.keys()).copied().collect();
                    self.error_help(E_NAME, pattern.span, format!("motif inconnu `{name}`"), suggest(name, known));
                    None
                };
                let Some(fields) = fields else { return };
                for (i, pf) in fields.iter().enumerate() {
                    let slot = match (&pf.name, &slots) {
                        (Some(n), Some(slots)) => {
                            let found = slots.iter().find(|s| s.name == n.name).copied();
                            if found.is_none() {
                                self.error_help(
                                    E_NAME,
                                    n.span,
                                    format!("`{name}` n'a pas de champ `{}`", n.name),
                                    suggest(&n.name, slots.iter().map(|s| s.name)),
                                );
                            }
                            found
                        }
                        (None, Some(slots)) => slots.get(i).copied(),
                        _ => None,
                    };
                    let ty = slot.and_then(|s| s.ty).map_or(Ty::Unknown, |t| self.peek_ty(t));
                    self.pattern(cx, &pf.pattern, &V { ty, taint: subject.taint });
                }
            }
            PatternKind::Array(items) => {
                let item = match subject.ty.base() {
                    Ty::Array(t) => (**t).clone(),
                    _ => Ty::Unknown,
                };
                for p in items {
                    self.pattern(cx, p, &V { ty: item.clone(), taint: subject.taint });
                }
            }
            PatternKind::Or(alts) => {
                for alt in alts {
                    self.pattern(cx, alt, subject);
                }
            }
        }
    }
}
