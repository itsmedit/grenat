//! Methods of `Int` and `Float`.

use crate::prelude::*;

use super::*;

pub(crate) fn int_method<'p>(interp: &mut Interp<'p>, n: i64, name: &str, args: &Args<'p>) -> Option<R<'p>> {
    let duration = |unit: f64| Some(Ok(Value::Duration(n as f64 * unit)));
    Some(match name {
        "times" => (|| {
            let body = block(args, name)?;
            for i in 0..n {
                interp.call_block(&body, vec![Value::Int(i)])?;
            }
            Ok(Value::Int(n))
        })(),
        "upto" => (|| {
            let (end, body) = (int_arg(args, 0, name)?, block(args, name)?);
            for i in n..=end {
                interp.call_block(&body, vec![Value::Int(i)])?;
            }
            Ok(Value::Int(n))
        })(),
        "to_i" | "round" | "floor" | "ceil" => Ok(Value::Int(n)),
        "to_f" => Ok(Value::Float(n as f64)),
        "abs" => Ok(Value::Int(n.abs())),
        "zero?" => Ok(Value::Bool(n == 0)),
        "even?" => Ok(Value::Bool(n % 2 == 0)),
        "odd?" => Ok(Value::Bool(n % 2 != 0)),
        "succ" => Ok(Value::Int(n + 1)),
        "pred" => Ok(Value::Int(n - 1)),
        "between?" => (|| {
            let (lo, hi) = (arg(args, 0, name)?, arg(args, 1, name)?);
            let v = Value::Int(n);
            Ok(Value::Bool(compare(&v, &lo).is_some_and(|o| o.is_ge()) && compare(&v, &hi).is_some_and(|o| o.is_le())))
        })(),
        "s" | "sec" | "second" | "seconds" => return duration(1.0),
        "min" | "minute" | "minutes" => return duration(60.0),
        "h" | "hour" | "hours" => return duration(3600.0),
        "day" | "days" => return duration(86_400.0),
        _ => return None,
    })
}

pub(crate) fn float_method<'p>(f: f64, name: &str, args: &Args<'p>) -> Option<R<'p>> {
    Some(match name {
        "round" => match args.pos.first() {
            Some(Value::Int(digits)) => {
                let factor = 10f64.powi(*digits as i32);
                Ok(Value::Float((f * factor).round() / factor))
            }
            _ => Ok(Value::Int(f.round() as i64)),
        },
        "floor" => Ok(Value::Int(f.floor() as i64)),
        "ceil" => Ok(Value::Int(f.ceil() as i64)),
        "to_i" => Ok(Value::Int(f as i64)),
        "to_f" => Ok(Value::Float(f)),
        "abs" => Ok(Value::Float(f.abs())),
        "zero?" => Ok(Value::Bool(f == 0.0)),
        "between?" => (|| {
            let (lo, hi) = (arg(args, 0, name)?, arg(args, 1, name)?);
            let v = Value::Float(f);
            Ok(Value::Bool(compare(&v, &lo).is_some_and(|o| o.is_ge()) && compare(&v, &hi).is_some_and(|o| o.is_le())))
        })(),
        _ => return None,
    })
}
