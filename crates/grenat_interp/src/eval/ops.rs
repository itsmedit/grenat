//! Opérateurs unaires et binaires, comparaisons.

use crate::prelude::*;

fn op_str(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Rem => "%",
        Pow => "**",
        Eq => "==",
        NotEq => "!=",
        Lt => "<",
        Le => "<=",
        Gt => ">",
        Ge => ">=",
        Cmp => "<=>",
        Match => "=~",
        And => "&&",
        Or => "||",
        BitAnd => "&",
        BitOr => "|",
        BitXor => "^",
        Shl => "<<",
        Shr => ">>",
    }
}

pub(crate) fn compare<'p>(a: &Value<'p>, b: &Value<'p>) -> Option<Ordering> {
    use Value::*;
    match (a.untainted(), b.untainted()) {
        (Int(x), Int(y)) => Some(x.cmp(y)),
        (Int(x), Float(y)) => (*x as f64).partial_cmp(y),
        (Float(x), Int(y)) => x.partial_cmp(&(*y as f64)),
        (Float(x), Float(y)) | (Money(x), Money(y)) | (Duration(x), Duration(y)) => x.partial_cmp(y),
        (Money(x), Float(y)) | (Duration(x), Float(y)) => x.partial_cmp(y),
        (Str(x), Str(y)) | (Symbol(x), Symbol(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

impl<'p> Interp<'p> {
    // ── Opérateurs ───────────────────────────────────────────

    pub(crate) fn binop(&mut self, op: BinOp, l: Value<'p>, r: Value<'p>) -> R<'p> {
        if l.is_tainted() || r.is_tainted() {
            let result = self.binop(op, l.untainted().clone(), r.untainted().clone())?;
            return Ok(result.taint());
        }
        use Value::*;
        let type_error = |l: &Value, r: &Value| {
            raise(
                "TypeError",
                format!("opérateur `{}` non défini entre {} et {}", op_str(op), l.type_name(), r.type_name()),
            )
        };
        match (op, &l, &r) {
            (BinOp::Eq, ..) => Ok(Bool(equal(&l, &r))),
            (BinOp::NotEq, ..) => Ok(Bool(!equal(&l, &r))),
            (BinOp::Shl, Array(items), _) => {
                items.borrow_mut().push(r.clone());
                Ok(l.clone())
            }
            (BinOp::Add, Str(a), Str(b)) => Ok(Value::str(format!("{a}{b}"))),
            (BinOp::Add, Str(_), _) => raise(
                "TypeError",
                format!("impossible d'ajouter {} à une chaîne : utilisez l'interpolation \"#{{…}}\"", r.type_name()),
            ),
            (BinOp::Mul, Str(s), Int(n)) if *n >= 0 => Ok(Value::str(s.repeat(*n as usize))),
            // copies d'abord : `xs + xs` verrouillerait deux fois le même tableau
            (BinOp::Add, Array(a), Array(b)) => {
                let (a, b) = (a.borrow().clone(), b.borrow().clone());
                Ok(Value::array(a.into_iter().chain(b).collect()))
            }
            (BinOp::Sub, Array(a), Array(b)) => {
                let (a, b) = (a.borrow().clone(), b.borrow().clone());
                Ok(Value::array(a.into_iter().filter(|x| !b.iter().any(|y| equal(x, y))).collect()))
            }
            (op, Int(a), Int(b)) => int_op(op, *a, *b).unwrap_or_else(|| type_error(&l, &r)),
            (op, Int(_) | Float(_), Int(_) | Float(_)) => {
                let as_f = |v: &Value| match v {
                    Int(n) => *n as f64,
                    Float(f) => *f,
                    _ => unreachable!(),
                };
                float_op(op, as_f(&l), as_f(&r)).unwrap_or_else(|| type_error(&l, &r))
            }
            (BinOp::Add, Money(a), Money(b)) => Ok(Money(a + b)),
            (BinOp::Sub, Money(a), Money(b)) => Ok(Money(a - b)),
            (BinOp::Add, Duration(a), Duration(b)) => Ok(Duration(a + b)),
            (BinOp::Mul, Duration(a), Int(n)) => Ok(Duration(a * *n as f64)),
            (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Cmp, ..) => match compare(&l, &r) {
                Some(ord) => Ok(match op {
                    BinOp::Lt => Bool(ord.is_lt()),
                    BinOp::Le => Bool(ord.is_le()),
                    BinOp::Gt => Bool(ord.is_gt()),
                    BinOp::Ge => Bool(ord.is_ge()),
                    _ => Int(ord as i64),
                }),
                None => type_error(&l, &r),
            },
            _ => type_error(&l, &r),
        }
    }
}

fn int_op<'p>(op: BinOp, a: i64, b: i64) -> Option<R<'p>> {
    use BinOp::*;
    let overflow = || raise("OverflowError", "dépassement d'entier");
    let checked = |r: Option<i64>| Some(r.map_or_else(overflow, |n| Ok(Value::Int(n))));
    match op {
        Add => checked(a.checked_add(b)),
        Sub => checked(a.checked_sub(b)),
        Mul => checked(a.checked_mul(b)),
        Div | Rem if b == 0 => Some(raise("ZeroDivisionError", "division par zéro")),
        // division entière arrondie vers -∞, comme en Ruby
        Div => checked(a.checked_div(b).map(|q| if (a % b != 0) && ((a < 0) != (b < 0)) { q - 1 } else { q })),
        Rem => checked(Some(((a % b) + b) % b)),
        Pow if b >= 0 => checked(u32::try_from(b).ok().and_then(|b| a.checked_pow(b))),
        Pow => Some(Ok(Value::Float((a as f64).powf(b as f64)))),
        Lt => Some(Ok(Value::Bool(a < b))),
        Le => Some(Ok(Value::Bool(a <= b))),
        Gt => Some(Ok(Value::Bool(a > b))),
        Ge => Some(Ok(Value::Bool(a >= b))),
        Cmp => Some(Ok(Value::Int(a.cmp(&b) as i64))),
        BitAnd => Some(Ok(Value::Int(a & b))),
        BitOr => Some(Ok(Value::Int(a | b))),
        BitXor => Some(Ok(Value::Int(a ^ b))),
        Shl => checked(u32::try_from(b).ok().and_then(|b| a.checked_shl(b))),
        Shr => checked(u32::try_from(b).ok().and_then(|b| a.checked_shr(b))),
        _ => None,
    }
}

fn float_op<'p>(op: BinOp, a: f64, b: f64) -> Option<R<'p>> {
    use BinOp::*;
    Some(Ok(match op {
        Add => Value::Float(a + b),
        Sub => Value::Float(a - b),
        Mul => Value::Float(a * b),
        Div => Value::Float(a / b),
        Rem => Value::Float(a.rem_euclid(b)),
        Pow => Value::Float(a.powf(b)),
        Lt => Value::Bool(a < b),
        Le => Value::Bool(a <= b),
        Gt => Value::Bool(a > b),
        Ge => Value::Bool(a >= b),
        Cmp => Value::Int(a.partial_cmp(&b).map_or(0, |o| o as i64)),
        _ => return None,
    }))
}
