//! Arrays as sets, as in Ruby: `a & b` and `intersection`, `a | b` and
//! `union`, `a - b` and `difference`, `intersect?`. The order is that of
//! the first array; `&` and `|` drop duplicates; items are compared with
//! `==`. An untrusted operand makes the result untrusted (`Interp::binop`).

use crate::prelude::*;

use super::*;

/// The items of `a` also in `b`, each once.
pub(crate) fn intersection<'p>(a: &[Value<'p>], b: &[Value<'p>]) -> Vec<Value<'p>> {
    distinct(a.iter().filter(|x| contains(b, x)).cloned())
}

/// The items of `a`, then those of `b`, each once.
pub(crate) fn union<'p>(a: &[Value<'p>], b: &[Value<'p>]) -> Vec<Value<'p>> {
    distinct(a.iter().chain(b).cloned())
}

/// The items of `a` not in `b` (duplicates kept, as in Ruby).
pub(crate) fn difference<'p>(a: &[Value<'p>], b: &[Value<'p>]) -> Vec<Value<'p>> {
    a.iter().filter(|x| !contains(b, x)).cloned().collect()
}

fn contains<'p>(items: &[Value<'p>], x: &Value<'p>) -> bool {
    items.iter().any(|y| equal(x, y))
}

fn distinct<'p>(items: impl Iterator<Item = Value<'p>>) -> Vec<Value<'p>> {
    let mut out: Vec<Value<'p>> = Vec::new();
    for item in items {
        if !contains(&out, &item) {
            out.push(item);
        }
    }
    out
}

/// `xs.intersection(ys, …)`, `union`, `difference`: the operator applied
/// to each argument in turn; `intersect?`: whether `xs & ys` has an item.
pub(crate) fn set_method<'p>(
    interp: &mut Interp<'p>,
    items: &Arc<Mutex<Vec<Value<'p>>>>,
    name: &str,
    args: &Args<'p>,
) -> Option<R<'p>> {
    let op = match name {
        "intersection" | "intersect?" => BinOp::BitAnd,
        "union" => BinOp::BitOr,
        "difference" => BinOp::Sub,
        _ => return None,
    };
    Some((|| {
        if name == "intersect?" {
            let common = interp.binop(op, Value::Array(items.clone()), arg(args, 0, name)?)?;
            let found = matches!(common.untainted(), Value::Array(xs) if !xs.borrow().is_empty());
            return Ok(if common.is_tainted() { Value::Bool(found).taint() } else { Value::Bool(found) });
        }
        // a copy, never `xs` itself; `xs.union` alone is `xs | []`
        let mut result = Value::array(items.borrow().clone());
        if args.pos.is_empty() && op == BinOp::BitOr {
            result = interp.binop(op, result, Value::array(Vec::new()))?;
        }
        for other in &args.pos {
            result = interp.binop(op, result, other.clone())?;
        }
        Ok(result)
    })())
}
