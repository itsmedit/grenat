//! Methods of `Array` (and `Range`, seen as an array); the others are in
//! `sequences`, those of `Hash` in `hashes`.

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
            None => match args.pos.first() {
                Some(x) => Ok(Value::Int(items.borrow().iter().filter(|i| equal(i, x)).count() as i64)),
                None => Ok(Value::Int(items.borrow().len() as i64)),
            },
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
                    return raise("TypeError", format!("`limit:` expects a positive integer, got {}", other.inspect()));
                }
            };
            interp.parallel_map(snapshot(), block(args, name)?, limit)
        })(),
        "batch_map" => (|| interp.batch_map(snapshot(), block(args, name)?))(),
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
        "index" | "find_index" if args.block.is_some() => {
            let (mut at, mut found) = (0, None);
            each(interp, &mut |_, r| {
                if r.truthy() {
                    found = Some(at);
                    return Ok(false);
                }
                at += 1;
                Ok(true)
            })
            .map(|()| found.map_or(Value::Nil, Value::Int))
        }
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
        // `xs.max(2)`: the two largest, largest first
        "min" | "max" if !args.pos.is_empty() => (|| {
            let n = int_arg(args, 0, name)?.max(0) as usize;
            let mut all = snapshot();
            let mut incomparable = false;
            all.sort_by(|a, b| {
                let o = compare(a, b).unwrap_or_else(|| {
                    incomparable = true;
                    Ordering::Equal
                });
                if name == "max" { o.reverse() } else { o }
            });
            if incomparable {
                return raise("TypeError", format!("`{name}`: values are not comparable"));
            }
            all.truncate(n);
            Ok(Value::array(all))
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
                        None => return Some(raise("TypeError", format!("`{name}`: values are not comparable"))),
                    },
                };
                if better {
                    best = Some(v);
                }
            }
            Ok(best.unwrap_or(Value::Nil))
        }
        "sort" if args.block.is_some() => (|| {
            let body = block(args, name)?;
            sort_with(interp, snapshot(), &body).map(Value::array)
        })(),
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
                return raise("TypeError", format!("`{name}`: values are not comparable"));
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
        "uniq" => (|| {
            let (mut out, mut keys): (Vec<Value<'p>>, Vec<Value<'p>>) = (Vec::new(), Vec::new());
            for v in snapshot() {
                let key = match &args.block {
                    Some(body) => interp.call_block(body, vec![v.clone()])?,
                    None => v.clone(),
                };
                if !keys.iter().any(|k| equal(k, &key)) {
                    keys.push(key);
                    out.push(v);
                }
            }
            Ok(Value::array(out))
        })(),
        "compact" => Ok(Value::array(snapshot().into_iter().filter(|v| !matches!(v, Value::Nil)).collect())),
        "take" => {
            int_arg(args, 0, name).map(|n| Value::array(snapshot().into_iter().take(n.max(0) as usize).collect()))
        }
        "drop" => {
            int_arg(args, 0, name).map(|n| Value::array(snapshot().into_iter().skip(n.max(0) as usize).collect()))
        }
        "zip" => (|| {
            let others = (0..args.pos.len().max(1)).map(|i| array_arg(args, i, name)).collect::<Result<Vec<_>, _>>()?;
            Ok(Value::array(
                snapshot()
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let mut row = vec![v];
                        row.extend(others.iter().map(|other| other.get(i).cloned().unwrap_or(Value::Nil)));
                        Value::array(row)
                    })
                    .collect(),
            ))
        })(),
        // `reduce(0) { |a, x| … }`, `reduce(:+)`, `inject(1, :*)`, `reduce(&:+)`
        "reduce" | "inject" => (|| {
            let (init, body) = match (args.pos.as_slice(), &args.block) {
                ([], Some(body)) => (None, body.clone()),
                ([init], Some(body)) => (Some(init.clone()), body.clone()),
                ([op], None) if matches!(op.untainted(), Value::Symbol(_)) => (None, op.untainted().clone()),
                ([init, op], None) if matches!(op.untainted(), Value::Symbol(_)) => {
                    (Some(init.clone()), op.untainted().clone())
                }
                _ => return raise("ArgumentError", format!("`{name}` expects a block or an operator (`{name}(:+)`)")),
            };
            let mut all = snapshot().into_iter();
            let mut acc = match init {
                Some(init) => init,
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
        _ => return sequence_method(interp, items, name, args),
    })
}

/// `sort { |a, b| … }`: the block compares, as `<=>` does (an `Int`).
fn sort_with<'p>(
    interp: &mut Interp<'p>,
    mut all: Vec<Value<'p>>,
    body: &Value<'p>,
) -> Result<Vec<Value<'p>>, Ctrl<'p>> {
    let mut failure: Option<Ctrl<'p>> = None;
    all.sort_by(|a, b| {
        if failure.is_some() {
            return Ordering::Equal;
        }
        let compared = interp.call_block(body, vec![a.clone(), b.clone()]).and_then(|v| match v.untainted() {
            Value::Int(n) => Ok(n.cmp(&0)),
            other => raise(
                "TypeError",
                format!("`sort`: the block compares as `<=>` does (an integer), got {}", other.type_name()),
            ),
        });
        compared.unwrap_or_else(|e| {
            failure = Some(e);
            Ordering::Equal
        })
    });
    match failure {
        Some(e) => Err(e),
        None => Ok(all),
    }
}
