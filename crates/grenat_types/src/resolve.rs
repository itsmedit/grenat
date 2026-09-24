//! Résolution des annotations de type, compatibilité, sérialisabilité pour les LLM.

use grenat_ast::{Field, Type, TypeKind};

use crate::ty::Ty;
use crate::*;

impl<'p> Checker<'p> {
    pub(crate) fn resolve(&mut self, ty: &'p Type) -> (Ty, bool) {
        match ty {
            Type::Tainted(inner, _) => (self.resolve(inner).0, true),
            Type::Optional(inner, _) => {
                let (inner, tainted) = self.resolve(inner);
                (Ty::opt(inner), tainted)
            }
            Type::Named { path, args, span } => {
                let name = path.last().expect("chemin non vide").name.as_str();
                let arg = |c: &mut Self, i: usize| args.get(i).map_or(Ty::Unknown, |a| c.resolve(a).0);
                let resolved = match name {
                    "Int" => Ty::Int,
                    "Float" => Ty::Float,
                    "String" | "Path" | "Email" | "Url" => Ty::Str,
                    "Symbol" => Ty::Sym,
                    "Bool" => Ty::Bool,
                    "Unit" | "Nil" => Ty::Nil,
                    "Money" => Ty::Money,
                    "Duration" => Ty::Duration,
                    "Range" => Ty::Range,
                    "Any" => Ty::Unknown,
                    "Array" => Ty::array(arg(self, 0)),
                    "Hash" => Ty::Hash(Box::new(arg(self, 0)), Box::new(arg(self, 1))),
                    "Result" => Ty::Result(Box::new(arg(self, 0)), Box::new(arg(self, 1))),
                    n if self.types.contains_key(n) || is_error_name(n) => Ty::user(n),
                    n => {
                        let known: Vec<&str> =
                            self.types.keys().copied().chain(builtins::TYPE_NAMES.iter().copied()).collect();
                        self.error_help(E_NAME, *span, format!("type inconnu `{n}`"), suggest(n, known));
                        Ty::Unknown
                    }
                };
                (resolved, false)
            }
        }
    }

    /// `actual` peut-il être passé là où `expected` est attendu ?
    pub(crate) fn compat(&self, actual: &Ty, expected: &Ty) -> bool {
        use Ty::*;
        match (actual, expected) {
            (Unknown, _) | (_, Unknown) => true,
            (a, b) if a == b => true,
            (Int, Float) | (Int | Float, Money) | (Sym, Str) => true,
            (Nil, Opt(_)) => true,
            (a, Opt(b)) => self.compat(a, b),
            // tolérant : `T?` accepté là où `T` est attendu (la phase 3 ajoutera la vérification de nil)
            (Opt(a), b) => self.compat(a, b),
            (Array(a), Array(b)) => self.compat(a, b),
            (Hash(k1, v1), Hash(k2, v2)) => self.compat(k1, k2) && self.compat(v1, v2),
            (Result(t1, e1), Result(t2, e2)) => self.compat(t1, t2) && self.compat(e1, e2),
            (User(a), User(b)) => {
                (is_error_name(a) && matches!(b.as_str(), "StandardError" | "Exception"))
                    || self.types.get(a.as_str()).is_some_and(|d| d.includes.contains(&b.as_str()))
            }
            (Range, Array(t)) => self.compat(&Int, t),
            _ => false,
        }
    }

    pub(crate) fn schema_ok(&self, ty: &Ty, depth: usize) -> Result<(), String> {
        if depth > 16 {
            return Err("type récursif : non représentable en JSON Schema".into());
        }
        match ty {
            Ty::Hash(..) => Err("`Hash` ne peut pas sortir d'un LLM : utilisez une `struct`".into()),
            Ty::Array(t) | Ty::Opt(t) => self.schema_ok(t, depth + 1),
            Ty::User(name) => {
                let Some(decl) = self.types.get(name.as_str()) else { return Ok(()) };
                let fields: Vec<&Field> = match decl.def.kind {
                    TypeKind::Struct => decl.fields.clone(),
                    TypeKind::Enum => decl.variants.iter().flat_map(|v| v.fields.iter()).collect(),
                    _ => return Err(format!("`{name}` ({:?}) ne peut pas sortir d'un LLM", decl.def.kind)),
                };
                for f in fields {
                    let Some(t) = &f.ty else { return Err(format!("le champ `{}` n'a pas de type", f.name.name)) };
                    self.schema_ok(&self.peek_ty(t), depth + 1)?;
                }
                Ok(())
            }
            Ty::Type(_) | Ty::Budget => Err(format!("{ty} ne peut pas sortir d'un LLM")),
            _ => Ok(()),
        }
    }

    /// Résolution sans diagnostic (pour les vérifications secondaires).
    pub(crate) fn peek_ty(&self, ty: &Type) -> Ty {
        match ty {
            Type::Tainted(inner, _) => self.peek_ty(inner),
            Type::Optional(inner, _) => Ty::opt(self.peek_ty(inner)),
            Type::Named { path, args, .. } => {
                let arg = |i: usize| args.get(i).map_or(Ty::Unknown, |a| self.peek_ty(a));
                match path.last().expect("chemin").name.as_str() {
                    "Int" => Ty::Int,
                    "Float" => Ty::Float,
                    "String" | "Path" | "Email" | "Url" => Ty::Str,
                    "Bool" => Ty::Bool,
                    "Array" => Ty::array(arg(0)),
                    "Hash" => Ty::Hash(Box::new(arg(0)), Box::new(arg(1))),
                    n if self.types.contains_key(n) => Ty::user(n),
                    _ => Ty::Unknown,
                }
            }
        }
    }
}
