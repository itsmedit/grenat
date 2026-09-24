//! Méthodes de `Array` et `Hash` (et `Range`, vu comme un tableau).

use crate::prelude::*;

use super::*;

pub(crate) fn array_method<'p>(
    interp: &mut Interp<'p>,
    items: &Arc<Mutex<Vec<Value<'p>>>>,
    name: &str,
    args: &Args<'p>,
) -> Option<R<'p>> {
    let snapshot = || items.borrow().clone();
    let each = |interp: &mut Interp<'p>, f: &mut dyn FnMut(Value<'p>, Value<'p>) -> Result<bool, Ctrl<'p>>| {
        let body = block(args, name)?;
        for item in snapshot() {
            let result = interp.call_block(&body, vec![item.clone()])?;
            if !f(item, result)? {
                break;
            }
        }
        Ok(())
    };
    Some(match name {
        "size" | "length" if args.block.is_none() => Ok(Value::Int(items.borrow().len() as i64)),
        "count" => match &args.block {
            None => Ok(Value::Int(items.borrow().len() as i64)),
            Some(_) => {
                let mut n = 0;
                each(interp, &mut |_, r| {
                    n += i64::from(r.truthy());
                    Ok(true)
                })
                .map(|()| Value::Int(n))
            }
        },
        "empty?" => Ok(Value::Bool(items.borrow().is_empty())),
        "first" => match args.pos.first() {
            Some(Value::Int(n)) => Ok(Value::array(snapshot().into_iter().take(*n as usize).collect())),
            _ => Ok(items.borrow().first().cloned().unwrap_or(Value::Nil)),
        },
        "last" => match args.pos.first() {
            Some(Value::Int(n)) => {
                let all = snapshot();
                Ok(Value::array(all[all.len().saturating_sub(*n as usize)..].to_vec()))
            }
            _ => Ok(items.borrow().last().cloned().unwrap_or(Value::Nil)),
        },
        "each" => each(interp, &mut |_, _| Ok(true)).map(|()| Value::Array(items.clone())),
        "each_with_index" => (|| {
            let body = block(args, name)?;
            for (i, item) in snapshot().into_iter().enumerate() {
                interp.call_block(&body, vec![item, Value::Int(i as i64)])?;
            }
            Ok(Value::Array(items.clone()))
        })(),
        "parallel_map" => (|| {
            let limit = match args.named.iter().find(|(n, _)| n == "limit").map(|(_, v)| v.untainted().clone()) {
                Some(Value::Int(n)) if n >= 1 => n as usize,
                None => 8,
                Some(other) => {
                    return raise("TypeError", format!("`limit:` attend un entier positif, reçu {}", other.inspect()));
                }
            };
            interp.parallel_map(snapshot(), block(args, name)?, limit)
        })(),
        "map" | "collect" => {
            let mut out = Vec::new();
            each(interp, &mut |_, r| {
                out.push(r);
                Ok(true)
            })
            .map(|()| Value::array(out))
        }
        "flat_map" => {
            let mut out = Vec::new();
            each(interp, &mut |_, r| {
                match r.untainted() {
                    Value::Array(inner) => out.extend(inner.borrow().iter().cloned()),
                    _ => out.push(r),
                }
                Ok(true)
            })
            .map(|()| Value::array(out))
        }
        "select" | "filter" | "reject" => {
            let keep = name != "reject";
            let mut out = Vec::new();
            each(interp, &mut |item, r| {
                if r.truthy() == keep {
                    out.push(item);
                }
                Ok(true)
            })
            .map(|()| Value::array(out))
        }
        "partition" => {
            let (mut yes, mut no) = (Vec::new(), Vec::new());
            each(interp, &mut |item, r| {
                if r.truthy() {
                    yes.push(item)
                } else {
                    no.push(item)
                }
                Ok(true)
            })
            .map(|()| Value::array(vec![Value::array(yes), Value::array(no)]))
        }
        "find" | "detect" => {
            let mut found = Value::Nil;
            each(interp, &mut |item, r| {
                if r.truthy() {
                    found = item;
                    return Ok(false);
                }
                Ok(true)
            })
            .map(|()| found)
        }
        "any?" | "all?" | "none?" => {
            let predicate = args.block.is_some();
            let results: Vec<bool> = if predicate {
                let mut out = Vec::new();
                if let Err(e) = each(interp, &mut |_, r| {
                    out.push(r.truthy());
                    Ok(true)
                }) {
                    return Some(Err(e));
                }
                out
            } else {
                snapshot().iter().map(Value::truthy).collect()
            };
            Ok(Value::Bool(match name {
                "any?" => results.iter().any(|b| *b),
                "all?" => results.iter().all(|b| *b),
                _ => !results.iter().any(|b| *b),
            }))
        }
        "include?" => arg(args, 0, name).map(|x| Value::Bool(items.borrow().iter().any(|i| equal(i, &x)))),
        "index" | "find_index" => arg(args, 0, name)
            .map(|x| items.borrow().iter().position(|i| equal(i, &x)).map_or(Value::Nil, |i| Value::Int(i as i64))),
        "sum" => (|| {
            let values = match &args.block {
                Some(body) => {
                    let body = body.clone();
                    snapshot().into_iter().map(|i| interp.call_block(&body, vec![i])).collect::<Result<Vec<_>, _>>()?
                }
                None => snapshot(),
            };
            let mut total = Value::Int(0);
            for v in values {
                total = interp.binop(grenat_ast::BinOp::Add, total, v)?;
            }
            Ok(total)
        })(),
        "min" | "max" => {
            let all = snapshot();
            let mut best: Option<Value<'p>> = None;
            for v in all {
                let better = match &best {
                    None => true,
                    Some(b) => match compare(&v, b) {
                        Some(o) => {
                            if name == "min" {
                                o.is_lt()
                            } else {
                                o.is_gt()
                            }
                        }
                        None => return Some(raise("TypeError", format!("`{name}` : valeurs non comparables"))),
                    },
                };
                if better {
                    best = Some(v);
                }
            }
            Ok(best.unwrap_or(Value::Nil))
        }
        "sort" | "sort_by" | "min_by" | "max_by" => (|| {
            let all = snapshot();
            let keys = match &args.block {
                Some(body) => {
                    let body = body.clone();
                    all.iter().map(|i| interp.call_block(&body, vec![i.clone()])).collect::<Result<Vec<_>, _>>()?
                }
                None => all.clone(),
            };
            let mut indexed: Vec<usize> = (0..all.len()).collect();
            let mut incomparable = false;
            indexed.sort_by(|&a, &b| {
                compare(&keys[a], &keys[b]).unwrap_or_else(|| {
                    incomparable = true;
                    std::cmp::Ordering::Equal
                })
            });
            if incomparable {
                return raise("TypeError", format!("`{name}` : valeurs non comparables"));
            }
            Ok(match name {
                "min_by" => indexed.first().map_or(Value::Nil, |&i| all[i].clone()),
                "max_by" => indexed.last().map_or(Value::Nil, |&i| all[i].clone()),
                _ => Value::array(indexed.into_iter().map(|i| all[i].clone()).collect()),
            })
        })(),
        "reverse" => Ok(Value::array(snapshot().into_iter().rev().collect())),
        "join" => (|| {
            let sep = match args.pos.first() {
                Some(v) => v.to_display(),
                None => String::new(),
            };
            let parts = snapshot().iter().map(|v| interp.display(v)).collect::<Result<Vec<_>, _>>()?;
            Ok(Value::str(parts.join(&sep)))
        })(),
        "push" | "append" => {
            items.borrow_mut().extend(args.pos.iter().cloned());
            Ok(Value::Array(items.clone()))
        }
        "pop" => Ok(items.borrow_mut().pop().unwrap_or(Value::Nil)),
        "shift" => {
            let mut items = items.borrow_mut();
            Ok(if items.is_empty() { Value::Nil } else { items.remove(0) })
        }
        "unshift" => {
            let mut all = args.pos.clone();
            all.extend(snapshot());
            *items.borrow_mut() = all;
            Ok(Value::Array(items.clone()))
        }
        "delete" => arg(args, 0, name).inspect(|x| {
            items.borrow_mut().retain(|i| !equal(i, x));
        }),
        "uniq" => {
            let mut out: Vec<Value<'p>> = Vec::new();
            for v in snapshot() {
                if !out.iter().any(|o| equal(o, &v)) {
                    out.push(v);
                }
            }
            Ok(Value::array(out))
        }
        "compact" => Ok(Value::array(snapshot().into_iter().filter(|v| !matches!(v, Value::Nil)).collect())),
        "take" => {
            int_arg(args, 0, name).map(|n| Value::array(snapshot().into_iter().take(n.max(0) as usize).collect()))
        }
        "drop" => {
            int_arg(args, 0, name).map(|n| Value::array(snapshot().into_iter().skip(n.max(0) as usize).collect()))
        }
        "zip" => (|| {
            let Value::Array(other) = arg(args, 0, name)? else {
                return raise("TypeError", "`zip` attend un tableau");
            };
            let other = other.borrow().clone();
            Ok(Value::array(
                snapshot()
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| Value::array(vec![v, other.get(i).cloned().unwrap_or(Value::Nil)]))
                    .collect(),
            ))
        })(),
        "reduce" | "inject" => (|| {
            let body = block(args, name)?;
            let mut all = snapshot().into_iter();
            let mut acc = match args.pos.first() {
                Some(init) => init.clone(),
                None => all.next().unwrap_or(Value::Nil),
            };
            for v in all {
                acc = interp.call_block(&body, vec![acc, v])?;
            }
            Ok(acc)
        })(),
        "group_by" | "tally" => (|| {
            let mut groups: Vec<(Value<'p>, Value<'p>)> = Vec::new();
            for item in snapshot() {
                let key = match &args.block {
                    Some(body) if name == "group_by" => interp.call_block(&body.clone(), vec![item.clone()])?,
                    _ => item.clone(),
                };
                let slot = groups.iter_mut().find(|(k, _)| equal(k, &key));
                match (name, slot) {
                    ("tally", Some((_, Value::Int(n)))) => *n += 1,
                    ("tally", _) => groups.push((key, Value::Int(1))),
                    (_, Some((_, Value::Array(list)))) => list.borrow_mut().push(item),
                    _ => groups.push((key, Value::array(vec![item]))),
                }
            }
            Ok(Value::Hash(Arc::new(Mutex::new(groups))))
        })(),
        "to_a" | "dup" => Ok(Value::array(snapshot())),
        _ => return None,
    })
}

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
        "size" | "length" | "count" => Ok(Value::Int(entries.borrow().len() as i64)),
        "empty?" => Ok(Value::Bool(entries.borrow().is_empty())),
        "keys" => Ok(Value::array(snapshot().into_iter().map(|(k, _)| k).collect())),
        "values" => Ok(Value::array(snapshot().into_iter().map(|(_, v)| v).collect())),
        "key?" | "has_key?" | "include?" => arg(args, 0, name).map(|k| Value::Bool(get(&k).is_some())),
        "fetch" => (|| {
            let key = arg(args, 0, name)?;
            match (get(&key), args.pos.get(1)) {
                (Some(v), _) => Ok(v),
                (None, Some(default)) => Ok(default.clone()),
                (None, None) => raise("KeyError", format!("clé absente : {}", key.inspect())),
            }
        })(),
        "delete" => arg(args, 0, name).map(|k| {
            let found = get(&k);
            entries.borrow_mut().retain(|(key, _)| !equal(key, &k));
            found.unwrap_or(Value::Nil)
        }),
        "merge" => (|| {
            let Value::Hash(other) = arg(args, 0, name)? else {
                return raise("TypeError", "`merge` attend un Hash");
            };
            let mut merged = snapshot();
            for (k, v) in other.borrow().iter() {
                match merged.iter_mut().find(|(key, _)| equal(key, k)) {
                    Some((_, slot)) => *slot = v.clone(),
                    None => merged.push((k.clone(), v.clone())),
                }
            }
            Ok(Value::Hash(Arc::new(Mutex::new(merged))))
        })(),
        "to_a" => Ok(pairs()),
        "each" | "map" | "select" | "filter" | "reject" | "any?" | "all?" | "find" | "sum" | "sort_by" | "min_by"
        | "max_by" => {
            let Value::Array(list) = pairs() else { unreachable!() };
            let result = array_method(interp, &list, name, args)?;
            // `select`/`reject` sur un Hash renvoient un Hash
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
