//! Extraction et validation des arguments des fonctions intégrées.

use crate::prelude::*;

pub(crate) fn arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<Value<'p>, Ctrl<'p>> {
    match args.pos.get(i) {
        Some(v) => Ok(v.clone()),
        None => raise("ArgumentError", format!("`{method}` attend au moins {} argument(s)", i + 1)),
    }
}

pub(crate) fn int_arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<i64, Ctrl<'p>> {
    match arg(args, i, method)?.untainted() {
        Value::Int(n) => Ok(*n),
        other => raise("TypeError", format!("`{method}` attend un entier, reçu {}", other.type_name())),
    }
}

pub(crate) fn str_arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<Arc<str>, Ctrl<'p>> {
    match arg(args, i, method)?.untainted() {
        Value::Str(s) | Value::Symbol(s) => Ok(s.clone()),
        other => raise("TypeError", format!("`{method}` attend une chaîne, reçu {}", other.type_name())),
    }
}

pub(crate) fn block<'p>(args: &Args<'p>, method: &str) -> Result<Value<'p>, Ctrl<'p>> {
    match &args.block {
        Some(b) => Ok(b.clone()),
        None => raise("ArgumentError", format!("`{method}` attend un bloc")),
    }
}

pub(crate) fn number(v: &Value) -> Option<f64> {
    match v.untainted() {
        Value::Int(n) => Some(*n as f64),
        Value::Float(f) | Value::Money(f) | Value::Duration(f) => Some(*f),
        _ => None,
    }
}

pub(crate) fn io_error<'p, T>(action: &str, path: &str, e: std::io::Error) -> Result<T, Ctrl<'p>> {
    raise("IoError", format!("{action} `{path}` : {e}"))
}
