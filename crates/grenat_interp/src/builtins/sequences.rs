//! More methods of `Array` (and `Range`): reshaping (`flatten`,
//! `each_slice`, `each_cons`, `rotate`, `product`, `to_h`, `slice`), and
//! Ruby's usual iterators (`filter_map`, `each_with_object`, `take_while`,
//! `drop_while`, `one?`, `minmax`, `dig`). An item of an untrusted array
//! stays untrusted wherever it lands.

use crate::prelude::*;

use super::*;

pub(crate) fn sequence_method<'p>(
    interp: &mut Interp<'p>,
    items: &Arc<Mutex<Vec<Value<'p>>>>,
    name: &str,
    args: &Args<'p>,
) -> Option<R<'p>> {
    let all = || items.borrow().clone();
    Some(match name {
        "flatten" => (|| {
            let depth = match args.pos.first() {
                Some(_) => Some(int_arg(args, 0, name)?),
                None => None,
            };
            let mut out = Vec::new();
            flatten_into(&mut out, all(), depth, false);
            Ok(Value::array(out))
        })(),
        "each_slice" | "each_cons" => (|| {
            let n = int_arg(args, 0, name)?;
            if n < 1 {
                return raise("ArgumentError", format!("`{name}` expects a positive size, got {n}"));
            }
            let all = all();
            let groups: Vec<Value<'p>> = if name == "each_slice" {
                all.chunks(n as usize).map(|c| Value::array(c.to_vec())).collect()
            } else {
                all.windows(n as usize).map(|w| Value::array(w.to_vec())).collect()
            };
            match &args.block {
                // with a block, as `each`: the receiver back
                Some(body) => {
                    for group in groups {
                        interp.call_block(body, vec![group])?;
                    }
                    Ok(Value::Array(items.clone()))
                }
                None => Ok(Value::array(groups)),
            }
        })(),
        "filter_map" => (|| {
            let body = block(args, name)?;
            let mut out = Vec::new();
            for item in all() {
                let v = interp.call_block(&body, vec![item])?;
                if v.truthy() {
                    out.push(v);
                }
            }
            Ok(Value::array(out))
        })(),
        "each_with_object" => (|| {
            let (memo, body) = (arg(args, 0, name)?, block(args, name)?);
            for item in all() {
                interp.call_block(&body, vec![item, memo.clone()])?;
            }
            Ok(memo)
        })(),
        "take_while" | "drop_while" => (|| {
            let body = block(args, name)?;
            let all = all();
            let mut cut = all.len();
            for (i, item) in all.iter().enumerate() {
                if !interp.call_block(&body, vec![item.clone()])?.truthy() {
                    cut = i;
                    break;
                }
            }
            Ok(Value::array(if name == "take_while" { all[..cut].to_vec() } else { all[cut..].to_vec() }))
        })(),
        "one?" => (|| {
            let mut found = 0;
            for item in all() {
                let yes = match &args.block {
                    Some(body) => interp.call_block(body, vec![item])?.truthy(),
                    None => item.truthy(),
                };
                found += usize::from(yes);
            }
            Ok(Value::Bool(found == 1))
        })(),
        "minmax" => (|| {
            let all = all();
            let mut bounds: Option<(Value<'p>, Value<'p>)> = None;
            for v in all {
                bounds = Some(match bounds {
                    None => (v.clone(), v),
                    Some((lo, hi)) => {
                        let (Some(below), Some(above)) = (compare(&v, &lo), compare(&v, &hi)) else {
                            return raise("TypeError", "`minmax`: values are not comparable");
                        };
                        (if below.is_lt() { v.clone() } else { lo }, if above.is_gt() { v } else { hi })
                    }
                });
            }
            let (lo, hi) = bounds.unwrap_or((Value::Nil, Value::Nil));
            Ok(Value::array(vec![lo, hi]))
        })(),
        "rotate" => (|| {
            let n = match args.pos.first() {
                Some(_) => int_arg(args, 0, name)?,
                None => 1,
            };
            let mut all = all();
            if !all.is_empty() {
                let k = n.rem_euclid(all.len() as i64) as usize;
                all.rotate_left(k);
            }
            Ok(Value::array(all))
        })(),
        "product" => (|| {
            let mut combos: Vec<Vec<Value<'p>>> = all().into_iter().map(|v| vec![v]).collect();
            for i in 0..args.pos.len() {
                let other = array_arg(args, i, name)?;
                combos = combos
                    .into_iter()
                    .flat_map(|combo| {
                        other.iter().map(move |v| {
                            let mut next = combo.clone();
                            next.push(v.clone());
                            next
                        })
                    })
                    .collect();
            }
            Ok(Value::array(combos.into_iter().map(Value::array).collect()))
        })(),
        "to_h" => (|| {
            let mut entries: Vec<(Value<'p>, Value<'p>)> = Vec::new();
            for item in all() {
                let pair = match &args.block {
                    Some(body) => interp.call_block(body, vec![item])?,
                    None => item,
                };
                let tainted = pair.is_tainted();
                let (k, v) = match pair.untainted() {
                    Value::Array(kv) if kv.borrow().len() == 2 => {
                        let kv = kv.borrow();
                        (kv[0].clone(), kv[1].clone())
                    }
                    other => {
                        return raise(
                            "TypeError",
                            format!("`to_h` expects [key, value] pairs, got {}", other.inspect()),
                        );
                    }
                };
                let (k, v) = if tainted { (k.taint(), v.taint()) } else { (k, v) };
                match entries.iter_mut().find(|(key, _)| equal(key, &k)) {
                    Some((_, slot)) => *slot = v,
                    None => entries.push((k, v)),
                }
            }
            Ok(Value::Hash(Arc::new(Mutex::new(entries))))
        })(),
        "slice" => (|| {
            let all = all();
            match super::slicing::bounds(&args.pos, all.len())? {
                Some(bounds) => Ok(super::slicing::of_array(&all, bounds)),
                None => interp.index(Value::Array(items.clone()), args.pos.clone()),
            }
        })(),
        "dig" => dig(interp, Value::Array(items.clone()), &args.pos),
        _ => return None,
    })
}

/// `flatten` (`depth` levels, all when `None`): the items of a nested
/// untrusted array are untrusted.
fn flatten_into<'p>(out: &mut Vec<Value<'p>>, items: Vec<Value<'p>>, depth: Option<i64>, tainted: bool) {
    for item in items {
        match item.untainted() {
            Value::Array(inner) if depth.is_none_or(|d| d > 0) => {
                let inner = inner.borrow().clone();
                flatten_into(out, inner, depth.map(|d| d - 1), tainted || item.is_tainted());
            }
            _ => out.push(if tainted { item.taint() } else { item }),
        }
    }
}

/// An array argument's items, untrusted when the array is.
pub(crate) fn array_arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<Vec<Value<'p>>, Ctrl<'p>> {
    let value = arg(args, i, method)?;
    match value.untainted() {
        Value::Array(items) => {
            let items = items.borrow().clone();
            Ok(if value.is_tainted() { items.into_iter().map(Value::taint).collect() } else { items })
        }
        other => raise("TypeError", format!("`{method}` expects an array, got {}", other.type_name())),
    }
}

/// `dig(k1, k2, …)`: a hash's key, an array's index, then the next one in
/// it; `nil` as soon as one is missing.
pub(crate) fn dig<'p>(interp: &mut Interp<'p>, mut value: Value<'p>, keys: &[Value<'p>]) -> R<'p> {
    if keys.is_empty() {
        return raise("ArgumentError", "`dig` expects at least one key");
    }
    for key in keys {
        value = match value.untainted() {
            Value::Nil => return Ok(Value::Nil),
            Value::Hash(_) | Value::Array(_) => interp.index(value, vec![key.clone()])?,
            other => return raise("TypeError", format!("`dig`: {} cannot be dug into", other.type_name())),
        };
    }
    Ok(value)
}
