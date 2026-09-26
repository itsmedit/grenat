//! Structural equality of values (identity for objects and agents).

use std::sync::Arc;

use super::*;

/// Structural equality (identity for objects).
pub fn equal<'p>(a: &Value<'p>, b: &Value<'p>) -> bool {
    use Value::*;
    match (a.untainted(), b.untainted()) {
        (Nil, Nil) => true,
        (Bool(x), Bool(y)) => x == y,
        (Int(x), Int(y)) => x == y,
        (Float(x), Float(y)) | (Money(x), Money(y)) | (Duration(x), Duration(y)) => x == y,
        (Int(x), Float(y)) | (Float(y), Int(x)) => (*x as f64) == *y,
        (Str(x), Str(y)) | (Symbol(x), Symbol(y)) | (Type(x), Type(y)) => x == y,
        // a token checked against a secret: the time does not tell how much matched
        (Secret(x), Secret(y) | Str(y)) | (Str(y), Secret(x)) => {
            x.len() == y.len() && x.bytes().zip(y.bytes()).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0
        }
        (Array(x), Array(y)) if Arc::ptr_eq(x, y) => true,
        (Hash(x), Hash(y)) if Arc::ptr_eq(x, y) => true,
        (Array(x), Array(y)) => {
            let (x, y) = (x.borrow(), y.borrow());
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| equal(a, b))
        }
        (Hash(x), Hash(y)) => {
            let (x, y) = (x.borrow(), y.borrow());
            x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| equal(k, k2) && equal(v, v2)))
        }
        (Range(a, b, c), Range(x, y, z)) => (a, b, c) == (x, y, z),
        (Record(x), Record(y)) => x.ty == y.ty && fields_equal(&x.fields, &y.fields),
        (Variant(x), Variant(y)) => {
            x.enum_name == y.enum_name && x.name == y.name && fields_equal(&x.fields, &y.fields)
        }
        (Object(x), Object(y)) => Arc::ptr_eq(x, y),
        (Closure(x), Closure(y)) => Arc::ptr_eq(x, y),
        (Error(x), Error(y)) => x.ty == y.ty && x.message == y.message,
        (Budget(x), Budget(y)) => Arc::ptr_eq(x, y),
        (Agent(x), Agent(y)) => Arc::ptr_eq(x, y),
        (Pool(x), Pool(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

pub(crate) fn fields_equal<'p>(a: &Fields<'p>, b: &Fields<'p>) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|((n1, v1), (n2, v2))| n1 == n2 && equal(v1, v2))
}
