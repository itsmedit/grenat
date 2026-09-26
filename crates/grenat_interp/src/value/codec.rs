//! Values as JSON, exactly: what a workflow's journal stores, read back with
//! the same types (a `Float` stays a `Float`, a struct keeps its type and
//! field order, a tainted value stays tainted).
//!
//! JSON's own values stand for themselves (`null`, booleans, integers,
//! strings, arrays); anything else is an object with one `$tag`.

use std::sync::{Arc, Mutex};

use serde_json::{Value as Json, json};

use super::*;

/// The JSON of `value`, or why it cannot be stored (an agent, an object…).
pub fn encode(value: &Value) -> Result<Json, String> {
    Ok(match value {
        Value::Nil => Json::Null,
        Value::Bool(b) => json!(b),
        Value::Int(n) => json!(n),
        Value::Str(s) => json!(&**s),
        Value::Array(items) => Json::Array(items.borrow().iter().map(encode).collect::<Result<_, _>>()?),
        Value::Float(f) => json!({"$float": f}),
        Value::Money(f) => json!({"$money": f}),
        Value::Duration(f) => json!({"$duration": f}),
        Value::Symbol(s) => json!({"$symbol": &**s}),
        Value::Type(s) => json!({"$type": &**s}),
        Value::Range(lo, hi, inclusive) => json!({"$range": [lo, hi, inclusive]}),
        Value::Hash(entries) => {
            let pairs = entries
                .borrow()
                .iter()
                .map(|(k, v)| Ok(json!([encode(k)?, encode(v)?])))
                .collect::<Result<Vec<_>, String>>()?;
            json!({"$hash": pairs})
        }
        Value::Record(r) => json!({"$record": &*r.ty, "fields": encode_fields(&r.fields)?}),
        Value::Variant(v) => json!({"$variant": [&*v.enum_name, &*v.name], "fields": encode_fields(&v.fields)?}),
        Value::Error(e) => json!({"$error": &*e.ty, "message": e.message, "fields": encode_fields(&e.fields)?}),
        Value::Tainted(inner) => json!({"$tainted": encode(inner)?}),
        Value::Secret(_) => return Err("a secret is never journaled nor queued: fetch it where it is used".into()),
        other => return Err(format!("{} is not data", other.type_name())),
    })
}

fn encode_fields(fields: &Fields) -> Result<Json, String> {
    Ok(Json::Array(fields.iter().map(|(n, v)| Ok(json!([&**n, encode(v)?]))).collect::<Result<_, String>>()?))
}

/// The value `json` stands for (see [`encode`]).
pub fn decode<'p>(json: &Json) -> Result<Value<'p>, String> {
    Ok(match json {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Int(n.as_i64().ok_or("an integer out of range")?),
        Json::String(s) => Value::str(s),
        Json::Array(items) => Value::array(items.iter().map(decode).collect::<Result<_, _>>()?),
        Json::Object(o) => {
            let (tag, payload) = o.iter().next().ok_or("an empty object")?;
            let number = || payload.as_f64().ok_or_else(|| format!("`{tag}` expects a number"));
            let text = || payload.as_str().map(Arc::<str>::from).ok_or_else(|| format!("`{tag}` expects a string"));
            match tag.as_str() {
                "$float" => Value::Float(number()?),
                "$money" => Value::Money(number()?),
                "$duration" => Value::Duration(number()?),
                "$symbol" => Value::Symbol(text()?),
                "$type" => Value::Type(text()?),
                "$range" => Value::Range(
                    payload[0].as_i64().ok_or("a range bound")?,
                    payload[1].as_i64().ok_or("a range bound")?,
                    payload[2].as_bool().unwrap_or(false),
                ),
                "$hash" => {
                    let pairs = payload.as_array().ok_or("`$hash` expects pairs")?;
                    let entries =
                        pairs.iter().map(|p| Ok((decode(&p[0])?, decode(&p[1])?))).collect::<Result<_, String>>()?;
                    Value::Hash(Arc::new(Mutex::new(entries)))
                }
                "$record" => Value::record(&text()?, decode_fields(&o["fields"])?),
                "$variant" => Value::Variant(Arc::new(Variant {
                    enum_name: payload[0].as_str().ok_or("a variant's enum")?.into(),
                    name: payload[1].as_str().ok_or("a variant's name")?.into(),
                    fields: decode_fields(&o["fields"])?,
                })),
                "$error" => {
                    let mut error = ErrorVal::new(&text()?, o["message"].as_str().unwrap_or_default());
                    error.fields = decode_fields(&o["fields"])?;
                    Value::Error(Arc::new(error))
                }
                "$tainted" => decode(payload)?.taint(),
                other => return Err(format!("unknown tag `{other}`")),
            }
        }
    })
}

fn decode_fields<'p>(json: &Json) -> Result<Fields<'p>, String> {
    json.as_array()
        .ok_or("fields expected")?
        .iter()
        .map(|pair| Ok((pair[0].as_str().ok_or("a field name")?.into(), decode(&pair[1])?)))
        .collect()
}
