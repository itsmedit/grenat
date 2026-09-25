//! Dispatch of built-in methods by value type.

use crate::prelude::*;

use super::*;

pub(crate) fn call_method<'p>(interp: &mut Interp<'p>, recv: Value<'p>, name: &str, args: Args<'p>) -> R<'p> {
    // shared by every value
    match name {
        "nil?" => return Ok(Value::Bool(matches!(recv, Value::Nil))),
        "to_s" => return interp.display(&recv).map(Value::str),
        "inspect" => return Ok(Value::str(recv.inspect())),
        "tainted?" => return Ok(Value::Bool(false)),
        // on an untainted value, validation is a simple pass-through
        "trust!" | "approve" => return Ok(recv),
        "check" => {
            let body = block(&args, name)?;
            return Ok(if interp.call_block(&body, vec![recv.clone()])?.truthy() {
                Value::ok(recv)
            } else {
                Value::err(Value::Error(Arc::new(ErrorVal::new(
                    "CheckError",
                    format!("validation failed for {}", recv.inspect()),
                ))))
            });
        }
        "is_a?" => {
            let Value::Type(ty) = arg(&args, 0, name)? else {
                return raise("TypeError", "`is_a?` expects a type");
            };
            return Ok(Value::Bool(interp.is_a(&recv, &ty)));
        }
        "class" => return Ok(Value::Type(recv.type_name().into())),
        _ => {}
    }
    let result = match &recv {
        Value::Int(n) => int_method(interp, *n, name, &args),
        Value::Float(f) => float_method(*f, name, &args),
        Value::Money(m) => match name {
            "to_f" => Some(Ok(Value::Float(*m))),
            _ => None,
        },
        Value::Duration(s) => match name {
            "seconds" | "to_f" => Some(Ok(Value::Float(*s))),
            "minutes" => Some(Ok(Value::Float(s / 60.0))),
            _ => None,
        },
        Value::Str(s) => str_method(s, name, &args),
        Value::Symbol(s) => match name {
            "to_sym" => Some(Ok(recv.clone())),
            "size" | "length" => Some(Ok(Value::Int(s.chars().count() as i64))),
            _ => None,
        },
        Value::Array(items) => array_method(interp, items, name, &args),
        Value::Hash(entries) => hash_method(interp, entries, name, &args),
        Value::Range(lo, hi, inclusive) => {
            let end = if *inclusive { *hi } else { hi - 1 };
            let items = Value::array((*lo..=end).map(Value::Int).collect());
            match name {
                "include?" | "cover?" => Some(int_arg(&args, 0, name).map(|n| Value::Bool(n >= *lo && n <= end))),
                "first" if args.pos.is_empty() => Some(Ok(Value::Int(*lo))),
                "last" if args.pos.is_empty() => Some(Ok(Value::Int(end))),
                "size" | "count" if args.block.is_none() => Some(Ok(Value::Int((end - lo + 1).max(0)))),
                _ => match &items {
                    Value::Array(items) => array_method(interp, items, name, &args),
                    _ => unreachable!(),
                },
            }
        }
        Value::Record(r) if &*r.ty == RESPONSE && matches!(name, "ok?" | "json") => response_method(&r.fields, name),
        Value::Record(r) if &*r.ty == DATABASE => Some(database_method(interp, &r.fields, name, args)),
        Value::Record(r) if &*r.ty == SHELL_RESULT && name == "ok?" => shell_result_method(&r.fields, name),
        Value::Record(r) => match name {
            "with" => Some((|| {
                let mut fields = r.fields.clone();
                for (field_name, value) in &args.named {
                    match fields.iter_mut().find(|(n, _)| &**n == field_name) {
                        Some((_, v)) => *v = value.clone(),
                        None => {
                            return raise("ArgumentError", format!("unknown field `{field_name}:` for `{}`", r.ty));
                        }
                    }
                }
                Ok(Value::record(&r.ty, fields))
            })()),
            "to_h" => Some(Ok(Value::Hash(Arc::new(Mutex::new(
                r.fields.iter().map(|(k, v)| (Value::Symbol(k.clone()), v.clone())).collect(),
            ))))),
            _ => None,
        },
        Value::Variant(v) if &*v.enum_name == "Result" => {
            let ok = &*v.name == "Ok";
            let payload = v.fields[0].1.clone();
            match name {
                "ok?" => Some(Ok(Value::Bool(ok))),
                "err?" => Some(Ok(Value::Bool(!ok))),
                "value" | "unwrap" if ok => Some(Ok(payload)),
                "value" | "unwrap" => Some(match payload {
                    Value::Error(e) => Err(Ctrl::Raise(e)),
                    other => raise("ResultError", other.to_display()),
                }),
                "error" => Some(Ok(if ok { Value::Nil } else { payload })),
                "unwrap_or" => Some(if ok { Ok(payload) } else { arg(&args, 0, name) }),
                "or_else" if ok => Some(Ok(payload)),
                "or_else" => Some(block(&args, name).and_then(|b| interp.call_block(&b, vec![payload]))),
                _ => None,
            }
        }
        Value::Variant(v) => match name {
            "name" => Some(Ok(Value::str(&*v.name))),
            _ => None,
        },
        Value::Error(e) => match name {
            "message" => Some(Ok(Value::str(&e.message))),
            "type" => Some(Ok(Value::str(&*e.ty))),
            "full_message" => Some(Ok(Value::str(format!("{}: {}", e.ty, e.message)))),
            _ => None,
        },
        Value::Budget(b) => match name {
            "spent" => Some(Ok(Value::Money(b.spent()))),
            "tokens" => Some(Ok(Value::Int(b.tokens() as i64))),
            "remaining" => Some(Ok(b.max_usd.map_or(Value::Nil, |max| Value::Money((max - b.spent()).max(0.0))))),
            _ => None,
        },
        Value::Closure(_) => match name {
            "call" => Some(interp.call_block(&recv, args.pos.clone())),
            _ => None,
        },
        _ => None,
    };
    result.unwrap_or_else(|| raise("NoMethodError", format!("unknown method `{name}` for {}", recv.type_name())))
}
