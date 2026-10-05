//! Methods of `Hash`: lookups (`fetch`, `dig`, `key?`), rebuilding
//! (`merge`, `transform_values`, `transform_keys`, `select`…), and the
//! iterators of `Array` over its `[key, value]` pairs.

use crate::prelude::*;

use super::*;

pub(crate) fn hash_method<'p>(
    interp: &mut Interp<'p>,
    entries: &Arc<Mutex<Vec<(Value<'p>, Value<'p>)>>>,
    name: &str,
    args: &Args<'p>,
) -> Option<R<'p>> {
    let snapshot = || entries.borrow().clone();
    let get = |key: &Value<'p>| entries.borrow().iter().find(|(k, _)| equal(k, key)).map(|(_, v)| v.clone());
    let pairs = || Value::array(snapshot().into_iter().map(|(k, v)| Value::array(vec![k, v])).collect());
    Some(match name {
        "size" | "length" | "count" if args.block.is_none() => Ok(Value::Int(entries.borrow().len() as i64)),
        "empty?" => Ok(Value::Bool(entries.borrow().is_empty())),
        "keys" => Ok(Value::array(snapshot().into_iter().map(|(k, _)| k).collect())),
        "values" => Ok(Value::array(snapshot().into_iter().map(|(_, v)| v).collect())),
        "key?" | "has_key?" | "include?" => arg(args, 0, name).map(|k| Value::Bool(get(&k).is_some())),
        "fetch" => (|| {
            let key = arg(args, 0, name)?;
            match (get(&key), args.pos.get(1), &args.block) {
                (Some(v), ..) => Ok(v),
                (None, Some(default), _) => Ok(default.clone()),
                (None, None, Some(body)) => interp.call_block(body, vec![key]),
                (None, None, None) => raise("KeyError", format!("missing key: {}", key.inspect())),
            }
        })(),
        "dig" => dig(interp, Value::Hash(entries.clone()), &args.pos),
        "to_h" => Ok(Value::Hash(Arc::new(Mutex::new(snapshot())))),
        "transform_values" | "transform_keys" => (|| {
            let body = block(args, name)?;
            let mut out: Vec<(Value<'p>, Value<'p>)> = Vec::new();
            for (k, v) in snapshot() {
                let (k, v) = if name == "transform_values" {
                    let v = interp.call_block(&body, vec![v])?;
                    (k, v)
                } else {
                    (interp.call_block(&body, vec![k])?, v)
                };
                match out.iter_mut().find(|(key, _)| equal(key, &k)) {
                    Some((_, slot)) => *slot = v,
                    None => out.push((k, v)),
                }
            }
            Ok(Value::Hash(Arc::new(Mutex::new(out))))
        })(),
        "delete" => arg(args, 0, name).map(|k| {
            let found = get(&k);
            entries.borrow_mut().retain(|(key, _)| !equal(key, &k));
            found.unwrap_or(Value::Nil)
        }),
        "merge" => (|| {
            let given = arg(args, 0, name)?;
            let Value::Hash(other) = given.untainted() else {
                return raise("TypeError", "`merge` expects a Hash");
            };
            let mut merged = snapshot();
            let mut other = other.borrow().clone();
            if given.is_tainted() {
                other = other.into_iter().map(|(k, v)| (k.taint(), v.taint())).collect();
            }
            for (k, v) in other {
                match merged.iter_mut().find(|(key, _)| equal(key, &k)) {
                    // `merge(other) { |key, old, new| … }` settles a conflict
                    Some((_, slot)) => match &args.block {
                        Some(body) => *slot = interp.call_block(body, vec![k, slot.clone(), v])?,
                        None => *slot = v,
                    },
                    None => merged.push((k, v)),
                }
            }
            Ok(Value::Hash(Arc::new(Mutex::new(merged))))
        })(),
        "to_a" => Ok(pairs()),
        "each" | "each_pair" | "map" | "flat_map" | "filter_map" | "select" | "filter" | "reject" | "any?" | "all?"
        | "none?" | "count" | "find" | "sum" | "sort_by" | "min_by" | "max_by" | "group_by" | "partition"
        | "each_with_object" => {
            let name = if name == "each_pair" { "each" } else { name };
            let Value::Array(list) = pairs() else { unreachable!() };
            let result = array_method(interp, &list, name, args)?;
            // `select`/`reject` on a Hash return a Hash
            Ok(match (name, result) {
                ("select" | "filter" | "reject", Ok(Value::Array(kept))) => {
                    let kept = kept
                        .borrow()
                        .iter()
                        .filter_map(|pair| match pair {
                            Value::Array(kv) => {
                                let kv = kv.borrow();
                                Some((kv[0].clone(), kv[1].clone()))
                            }
                            _ => None,
                        })
                        .collect();
                    Value::Hash(Arc::new(Mutex::new(kept)))
                }
                ("each", Ok(_)) => Value::Hash(entries.clone()),
                (_, result) => return Some(result),
            })
        }
        _ => return None,
    })
}
