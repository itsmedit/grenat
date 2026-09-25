//! Extraction and validation of built-in function arguments.

use crate::prelude::*;

pub(crate) fn arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<Value<'p>, Ctrl<'p>> {
    match args.pos.get(i) {
        Some(v) => Ok(v.clone()),
        None => raise("ArgumentError", format!("`{method}` expects at least {} argument(s)", i + 1)),
    }
}

pub(crate) fn int_arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<i64, Ctrl<'p>> {
    match arg(args, i, method)?.untainted() {
        Value::Int(n) => Ok(*n),
        other => raise("TypeError", format!("`{method}` expects an integer, got {}", other.type_name())),
    }
}

pub(crate) fn str_arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<Arc<str>, Ctrl<'p>> {
    match arg(args, i, method)?.untainted() {
        Value::Str(s) | Value::Symbol(s) => Ok(s.clone()),
        other => raise("TypeError", format!("`{method}` expects a string, got {}", other.type_name())),
    }
}

pub(crate) fn block<'p>(args: &Args<'p>, method: &str) -> Result<Value<'p>, Ctrl<'p>> {
    match &args.block {
        Some(b) => Ok(b.clone()),
        None => raise("ArgumentError", format!("`{method}` expects a block")),
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
    raise("IoError", format!("{action} `{path}`: {e}"))
}

/// The text of a string literal without interpolation.
pub(crate) fn literal_text(e: &grenat_ast::Expr) -> Option<String> {
    match &e.kind {
        grenat_ast::ExprKind::Str(segments) => segments
            .iter()
            .map(|s| match s {
                grenat_ast::StrSeg::Lit(t) => Some(t.as_str()),
                grenat_ast::StrSeg::Interp(_) => None,
            })
            .collect(),
        _ => None,
    }
}
