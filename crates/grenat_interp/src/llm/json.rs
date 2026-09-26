//! Conversion between JSON (LLM replies, tool arguments) and Grenat values.

use crate::prelude::*;
use serde_json::{Map, Value as Json, json};

use super::*;

impl<'p> Interp<'p> {
    // ── JSON ↔ values ───────────────────────────────────────

    pub(crate) fn json_to_value(&mut self, json: &Json, ty: &Ty<'p>) -> Result<Value<'p>, String> {
        let mismatch = |expected: &str| Err(format!("{expected}, got {json}"));
        match ty {
            Ty::Int => match json.as_i64().or_else(|| json.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)) {
                Some(n) => Ok(Value::Int(n)),
                None => mismatch("expected an integer"),
            },
            Ty::Float => json.as_f64().map(Value::Float).map_or_else(|| mismatch("expected a number"), Ok),
            Ty::Str => json.as_str().map(Value::str).map_or_else(|| mismatch("expected a string"), Ok),
            Ty::Bool => json.as_bool().map(Value::Bool).map_or_else(|| mismatch("expected a boolean"), Ok),
            Ty::Nil => Ok(Value::Nil),
            Ty::Opt(inner) => {
                if json.is_null() {
                    Ok(Value::Nil)
                } else {
                    self.json_to_value(json, inner)
                }
            }
            Ty::Array(item) => {
                let Some(items) = json.as_array() else { return mismatch("expected an array") };
                let values = items.iter().map(|i| self.json_to_value(i, item)).collect::<Result<_, _>>()?;
                Ok(Value::array(values))
            }
            Ty::Hash(value_ty) => {
                let Some(object) = json.as_object() else { return mismatch("expected an object") };
                let mut pairs = Vec::new();
                for (k, v) in object {
                    pairs.push((Value::str(k), self.json_to_value(v, value_ty)?));
                }
                Ok(Value::Hash(Arc::new(std::sync::Mutex::new(pairs))))
            }
            Ty::User(name) => self.json_to_user(json, name),
        }
    }

    pub(crate) fn json_to_user(&mut self, json: &Json, name: &'p str) -> Result<Value<'p>, String> {
        let info = &self.types[name];
        match info.def.kind {
            TypeKind::Struct => {
                let defs = info.fields.clone();
                let object = json.as_object().ok_or_else(|| format!("expected a `{name}` object, got {json}"))?;
                let fields = self.json_fields(object, &defs, name)?;
                Ok(Value::record(name, fields))
            }
            TypeKind::Enum => {
                let variants = info.variants.clone();
                let (variant_name, object) = match json {
                    Json::String(s) => (s.as_str(), None),
                    Json::Object(o) => (o.get("kind").and_then(Json::as_str).unwrap_or_default(), Some(o)),
                    _ => return Err(format!("expected a variant of `{name}`, got {json}")),
                };
                let variant = variants
                    .iter()
                    .find(|v| v.name.name == variant_name)
                    .or_else(|| variants.iter().find(|v| v.name.name.eq_ignore_ascii_case(variant_name)))
                    .ok_or_else(|| format!("`{variant_name}` is not a variant of `{name}`"))?;
                let defs: Vec<_> = variant.fields.iter().collect();
                let fields = match object {
                    Some(o) if !defs.is_empty() => self.json_fields(o, &defs, &variant.name.name)?,
                    _ => Vec::new(),
                };
                Ok(Value::Variant(Arc::new(Variant {
                    enum_name: name.into(),
                    name: variant.name.name.as_str().into(),
                    fields,
                })))
            }
            kind => Err(format!("a {kind:?} cannot be built from JSON")),
        }
    }

    pub(crate) fn json_fields(
        &mut self,
        object: &Map<String, Json>,
        defs: &[&'p grenat_ast::Field],
        owner: &str,
    ) -> Result<Fields<'p>, String> {
        let mut fields = Vec::with_capacity(defs.len());
        for def in defs {
            let ty = def.ty.as_ref().ok_or_else(|| format!("field `{}` has no type", def.name.name))?;
            let ty = self.ty(ty)?;
            let value = match object.get(&def.name.name) {
                None | Some(Json::Null) if def.default.is_some() => {
                    let default = def.default.as_ref().expect("checked above");
                    self.eval(default).map_err(|_| format!("invalid default value for `{}`", def.name.name))?
                }
                None if matches!(ty, Ty::Opt(_)) => Value::Nil,
                None => return Err(format!("missing field `{}` for `{owner}`", def.name.name)),
                Some(json) => self.json_to_value(json, &ty).map_err(|e| format!("`{owner}.{}`: {e}", def.name.name))?,
            };
            fields.push((def.name.name.as_str().into(), value));
        }
        Ok(fields)
    }
}

/// JSON representation of a value (tool results, `Json.dump`): secrets
/// show as `[secret]`.
pub(crate) fn value_to_json(value: &Value) -> Json {
    to_json(value, false)
}

/// As [`value_to_json`], secrets revealed: a request body sent where they
/// serve (`Http.post(url, json: {…})`).
pub(crate) fn revealed_json(value: &Value) -> Json {
    to_json(value, true)
}

fn to_json(value: &Value, reveal: bool) -> Json {
    let json = |v: &Value| to_json(v, reveal);
    match value.untainted() {
        Value::Nil => Json::Null,
        Value::Secret(text) if reveal => json!(&**text),
        Value::Bool(b) => json!(b),
        Value::Int(n) => json!(n),
        Value::Float(f) | Value::Money(f) | Value::Duration(f) => json!(f),
        Value::Str(s) | Value::Symbol(s) | Value::Type(s) => json!(&**s),
        Value::Array(items) => Json::Array(items.borrow().iter().map(json).collect()),
        Value::Hash(entries) => Json::Object(entries.borrow().iter().map(|(k, v)| (k.to_display(), json(v))).collect()),
        Value::Range(lo, hi, inclusive) => {
            let hi = if *inclusive { *hi } else { hi - 1 };
            Json::Array((*lo..=hi).map(|n| json!(n)).collect())
        }
        Value::Record(r) => fields_to_json(&r.fields, reveal),
        Value::Variant(v) if v.fields.is_empty() => json!(&*v.name),
        Value::Variant(v) => {
            let mut object = fields_to_json(&v.fields, reveal);
            object["kind"] = json!(&*v.name);
            object
        }
        Value::Error(e) => json!({"error": &*e.ty, "message": e.message}),
        other => json!(other.inspect()),
    }
}

fn fields_to_json(fields: &Fields, reveal: bool) -> Json {
    Json::Object(fields.iter().map(|(k, v)| (k.to_string(), to_json(v, reveal))).collect())
}
