//! Types vus par les LLM et JSON Schema générés depuis les déclarations.

use crate::prelude::*;
use grenat_ast::Type;
use serde_json::{Map, Value as Json, json};

pub(crate) const MAX_SCHEMA_DEPTH: usize = 16;

/// Type tel que vu par les LLM (la teinte `~` est ajoutée par le runtime).
#[derive(Debug, Clone)]
pub(crate) enum Ty<'p> {
    Int,
    Float,
    Str,
    Bool,
    Nil,
    Array(Box<Ty<'p>>),
    Hash(Box<Ty<'p>>),
    Opt(Box<Ty<'p>>),
    User(&'p str),
}

pub(crate) fn type_name(ty: &Type) -> &str {
    match ty {
        Type::Named { path, .. } => &path.last().expect("chemin non vide").name,
        Type::Optional(inner, _) | Type::Tainted(inner, _) => type_name(inner),
    }
}

pub(crate) fn type_error<'p, T>(message: String) -> Result<T, Ctrl<'p>> {
    raise("TypeError", message)
}

impl<'p> Interp<'p> {
    // ── Types et schémas ─────────────────────────────────────

    pub(crate) fn ty(&self, ty: &'p Type) -> Result<Ty<'p>, String> {
        match ty {
            Type::Tainted(inner, _) => self.ty(inner),
            Type::Optional(inner, _) => Ok(Ty::Opt(Box::new(self.ty(inner)?))),
            Type::Named { path, args, .. } => {
                let name = path.last().expect("chemin non vide").name.as_str();
                let arg = |i: usize| -> Result<Ty<'p>, String> {
                    args.get(i)
                        .map(|a| self.ty(a))
                        .unwrap_or_else(|| Err(format!("`{name}` attend un type en paramètre")))
                };
                Ok(match name {
                    "Int" => Ty::Int,
                    "Float" | "Money" => Ty::Float,
                    "String" | "Path" | "Email" | "Url" | "Symbol" => Ty::Str,
                    "Bool" => Ty::Bool,
                    "Unit" | "Nil" => Ty::Nil,
                    "Array" => Ty::Array(Box::new(arg(0)?)),
                    "Hash" => Ty::Hash(Box::new(arg(1)?)),
                    _ => match self.types.get(name) {
                        Some(info) => Ty::User(info.def.name.name.as_str()),
                        None => return Err(format!("type `{name}` inconnu")),
                    },
                })
            }
        }
    }

    pub(crate) fn schema(&self, ty: &Ty<'p>, depth: usize) -> Result<Json, String> {
        if depth > MAX_SCHEMA_DEPTH {
            return Err("type récursif : non représentable en JSON Schema".into());
        }
        Ok(match ty {
            Ty::Int => json!({"type": "integer"}),
            Ty::Float => json!({"type": "number"}),
            Ty::Str => json!({"type": "string"}),
            Ty::Bool => json!({"type": "boolean"}),
            Ty::Nil => json!({"type": "null"}),
            Ty::Array(item) => json!({"type": "array", "items": self.schema(item, depth + 1)?}),
            Ty::Opt(inner) => json!({"anyOf": [self.schema(inner, depth + 1)?, {"type": "null"}]}),
            Ty::Hash(_) => return Err("`Hash` ne peut pas sortir d'un LLM : utilisez une `struct`".into()),
            Ty::User(name) => {
                let info = &self.types[name];
                let mut schema = match info.def.kind {
                    TypeKind::Struct => {
                        let fields: Vec<_> =
                            info.fields.iter().map(|f| (*f, f.ty.as_ref(), f.default.is_some())).collect();
                        self.object_schema(
                            fields.into_iter().map(|(f, ty, opt)| (f.name.name.as_str(), ty, f.doc.as_deref(), opt)),
                            depth,
                        )?
                    }
                    TypeKind::Enum if info.variants.iter().all(|v| v.fields.is_empty()) => {
                        let names: Vec<_> = info.variants.iter().map(|v| v.name.name.as_str()).collect();
                        let mut s = json!({"type": "string", "enum": names});
                        let docs: Vec<String> = info
                            .variants
                            .iter()
                            .filter_map(|v| v.doc.as_ref().map(|d| format!("{} : {d}", v.name.name)))
                            .collect();
                        if !docs.is_empty() {
                            s["description"] = json!(docs.join("\n"));
                        }
                        s
                    }
                    TypeKind::Enum => {
                        let mut alternatives = Vec::new();
                        for variant in &info.variants {
                            let fields = variant
                                .fields
                                .iter()
                                .map(|f| (f.name.name.as_str(), f.ty.as_ref(), f.doc.as_deref(), f.default.is_some()));
                            let mut object = self.object_schema(fields, depth)?;
                            object["properties"]["kind"] = json!({"type": "string", "enum": [variant.name.name]});
                            object["required"].as_array_mut().expect("tableau").insert(0, json!("kind"));
                            alternatives.push(object);
                        }
                        json!({"anyOf": alternatives})
                    }
                    kind => return Err(format!("un {kind:?} (`{name}`) ne peut pas sortir d'un LLM")),
                };
                if let Some(doc) = &info.def.doc {
                    schema["description"] = json!(doc);
                }
                schema
            }
        })
    }

    /// Objet strict : tous les champs requis, ceux qui ont une valeur par défaut acceptent `null`.
    pub(crate) fn object_schema<'a>(
        &self,
        fields: impl Iterator<Item = (&'a str, Option<&'p Type>, Option<&'a str>, bool)>,
        depth: usize,
    ) -> Result<Json, String> {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for (name, ty, doc, has_default) in fields {
            let ty = ty.ok_or_else(|| format!("le champ `{name}` n'a pas de type"))?;
            let mut schema = self.schema(&self.ty(ty)?, depth + 1)?;
            if has_default {
                schema = json!({"anyOf": [schema, {"type": "null"}]});
            }
            if let Some(doc) = doc {
                schema["description"] = json!(doc);
            }
            properties.insert(name.to_string(), schema);
            required.push(json!(name));
        }
        Ok(json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}))
    }

    /// Schéma de sortie : un objet, en enveloppant `{"value": …}` si nécessaire.
    pub(crate) fn output_schema(&self, ty: &Ty<'p>) -> Result<(Json, bool), String> {
        let schema = self.schema(ty, 0)?;
        if schema["type"] == "object" {
            return Ok((schema, false));
        }
        let wrapped = json!({
            "type": "object",
            "properties": {"value": schema},
            "required": ["value"],
            "additionalProperties": false,
        });
        Ok((wrapped, true))
    }
}
