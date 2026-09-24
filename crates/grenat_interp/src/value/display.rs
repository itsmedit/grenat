//! Text representations: `puts` (to_display) and `p` (inspect).

use std::fmt::{self, Write};

use super::*;

pub fn money(usd: f64) -> String {
    if usd < 0.01 && usd > 0.0 { format!("${usd:.4}") } else { format!("${usd:.2}") }
}

pub fn duration(seconds: f64) -> String {
    if seconds >= 86_400.0 && seconds % 86_400.0 == 0.0 {
        format!("{}d", seconds / 86_400.0)
    } else if seconds >= 3600.0 && seconds % 3600.0 == 0.0 {
        format!("{}h", seconds / 3600.0)
    } else if seconds >= 60.0 && seconds % 60.0 == 0.0 {
        format!("{}min", seconds / 60.0)
    } else {
        format!("{seconds}s")
    }
}

/// Shared with native code, which must print numbers identically.
pub(crate) use grenat_runtime::format_float as float;

impl<'p> Value<'p> {
    /// Type name, for error messages and `is_a?`.
    pub fn type_name(&self) -> String {
        match self {
            Value::Nil => "Nil".into(),
            Value::Bool(_) => "Bool".into(),
            Value::Int(_) => "Int".into(),
            Value::Float(_) => "Float".into(),
            Value::Money(_) => "Money".into(),
            Value::Duration(_) => "Duration".into(),
            Value::Str(_) => "String".into(),
            Value::Symbol(_) => "Symbol".into(),
            Value::Array(_) => "Array".into(),
            Value::Hash(_) => "Hash".into(),
            Value::Range(..) => "Range".into(),
            Value::Record(r) => r.ty.to_string(),
            Value::Object(o) => o.ty.to_string(),
            Value::Variant(v) => v.enum_name.to_string(),
            Value::Closure(_) => "Block".into(),
            Value::Type(_) => "Type".into(),
            Value::Error(e) => e.ty.to_string(),
            Value::Budget(_) => "Budget".into(),
            Value::Agent(a) => a.ty.to_string(),
            Value::Pool(p) => p.first().map_or("Pool".into(), |a| a.ty.to_string()),
            Value::Tainted(inner) => format!("~{}", inner.type_name()),
        }
    }

    /// Representation for `puts` and interpolation.
    pub fn to_display(&self) -> String {
        match self {
            Value::Nil => String::new(),
            Value::Str(s) | Value::Symbol(s) => s.to_string(),
            Value::Tainted(inner) => inner.to_display(),
            Value::Error(e) => e.message.clone(),
            _ => self.inspect(),
        }
    }

    /// Representation for `p` and debug messages.
    pub fn inspect(&self) -> String {
        let mut out = String::new();
        self.write_inspect(&mut out);
        out
    }

    fn write_inspect(&self, out: &mut String) {
        let fields = |out: &mut String, fields: &Fields<'p>| {
            out.push('(');
            for (i, (name, value)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{name}: ");
                value.write_inspect(out);
            }
            out.push(')');
        };
        match self {
            Value::Nil => out.push_str("nil"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(n) => out.push_str(&n.to_string()),
            Value::Float(f) => out.push_str(&float(*f)),
            Value::Money(m) => out.push_str(&money(*m)),
            Value::Duration(d) => out.push_str(&duration(*d)),
            Value::Str(s) => {
                let _ = write!(out, "\"{}\"", s.escape_debug());
            }
            Value::Symbol(s) => {
                let _ = write!(out, ":{s}");
            }
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.write_inspect(out);
                }
                out.push(']');
            }
            Value::Hash(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    match k {
                        Value::Symbol(s) => {
                            let _ = write!(out, "{s}: ");
                        }
                        other => {
                            other.write_inspect(out);
                            out.push_str(" => ");
                        }
                    }
                    v.write_inspect(out);
                }
                out.push('}');
            }
            Value::Range(lo, hi, inclusive) => {
                let _ = write!(out, "{lo}{}{hi}", if *inclusive { ".." } else { "..." });
            }
            Value::Record(r) => {
                out.push_str(&r.ty);
                fields(out, &r.fields);
            }
            Value::Object(o) => {
                let _ = write!(out, "#<{}", o.ty);
                for (name, value) in o.fields.borrow().iter() {
                    let _ = write!(out, " @{name}=");
                    value.write_inspect(out);
                }
                out.push('>');
            }
            Value::Variant(v) => {
                out.push_str(&v.name);
                if !v.fields.is_empty() {
                    fields(out, &v.fields);
                }
            }
            Value::Closure(_) => out.push_str("#<Block>"),
            Value::Type(name) => out.push_str(name),
            Value::Error(e) => {
                let _ = write!(out, "{}(\"{}\")", e.ty, e.message.escape_debug());
            }
            Value::Budget(b) => {
                let _ = write!(out, "#<Budget {} / {} tokens>", money(b.spent()), b.tokens());
            }
            Value::Agent(a) => {
                let _ = write!(out, "#<Agent {} {}>", a.ty, a.id);
            }
            Value::Pool(p) => {
                let _ = write!(out, "#<Pool {}×{}>", p.first().map_or("?", |a| &*a.ty), p.len());
            }
            Value::Tainted(inner) => {
                out.push('~');
                inner.write_inspect(out);
            }
        }
    }
}

impl fmt::Debug for Value<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.inspect())
    }
}
