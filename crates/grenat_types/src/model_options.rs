//! The values of a model declaration's options, checked where they are
//! written as literals — as the interpreter takes them when the program
//! loads (`grenat_interp`'s `program.rs`), so that `grenat check` passes
//! no declaration `grenat run` would refuse. A value computed (a variable,
//! a call) is the interpreter's to check.

use grenat_ast::{Expr, ExprKind, StrSeg, UnOp};

/// A literal, as far as the checker can read it.
#[derive(Debug, Clone, PartialEq)]
enum Lit {
    Int(i64),
    Float(f64),
    /// A string without interpolation, or a symbol: the interpreter takes
    /// either where it takes a name.
    Text(String),
    Bool,
    Nil,
    Hash,
    Array,
    /// Computed: known only when the program runs.
    Computed,
}

fn lit(expr: &Expr) -> Lit {
    match &expr.kind {
        ExprKind::Int(n) => Lit::Int(*n),
        ExprKind::Float(f) => Lit::Float(*f),
        ExprKind::Unary { op: UnOp::Neg, expr } => match lit(expr) {
            Lit::Int(n) => Lit::Int(-n),
            Lit::Float(f) => Lit::Float(-f),
            _ => Lit::Computed,
        },
        ExprKind::Symbol(s) => Lit::Text(s.clone()),
        ExprKind::Str(segments) => match segments.as_slice() {
            [] => Lit::Text(String::new()),
            [StrSeg::Lit(text)] => Lit::Text(text.clone()),
            _ => Lit::Computed,
        },
        ExprKind::Bool(_) => Lit::Bool,
        ExprKind::Nil => Lit::Nil,
        ExprKind::Hash(_) => Lit::Hash,
        ExprKind::Array(_) => Lit::Array,
        _ => Lit::Computed,
    }
}

/// What is wrong with `value` as option `name`, when it is written as a
/// literal the interpreter refuses. `kind` is the model's (`chat`,
/// `embedding`, `transcription`).
pub(crate) fn invalid(name: &str, value: &Expr, kind: grenat_llm::ModelKind) -> Option<String> {
    let value_lit = lit(value);
    if value_lit == Lit::Computed {
        return None;
    }
    let ok = match name {
        "provider" | "effort" => matches!(value_lit, Lit::Text(_)),
        "name" => matches!(value, Expr { kind: ExprKind::Str(_), .. }),
        "temperature" => matches!(value_lit, Lit::Int(_) | Lit::Float(_)),
        "max_tokens" => matches!(value_lit, Lit::Int(n) if n > 0),
        "fallbacks" => value_lit == Lit::Bool,
        "base_url" => matches!(value, Expr { kind: ExprKind::Str(_), .. }),
        "kind" => matches!(&value_lit, Lit::Text(k) if ["chat", "embedding", "transcription"].contains(&k.as_str())),
        "dimensions" => matches!(value_lit, Lit::Int(n) if n > 0 && n <= i64::from(u32::MAX)),
        "cache" => {
            value_lit == Lit::Bool
                || matches!((&value.kind, &value_lit), (ExprKind::Symbol(_), Lit::Text(c)) if c == "agents")
        }
        "cache_ttl" => matches!(&value_lit, Lit::Text(t) if t == "5m" || t == "1h"),
        "price" => return price(value, kind),
        _ => true,
    };
    (!ok).then(|| expected(name).to_string())
}

/// What option `name` takes.
fn expected(name: &str) -> &'static str {
    match name {
        "provider" => "`provider:` is a provider's name (`:anthropic`)",
        "effort" => "`effort:` is a level (`:high`)",
        "name" => "`name:` is the model's name, a string (`\"claude-haiku-4-5\"`)",
        "temperature" => "`temperature:` is a number",
        "max_tokens" => "`max_tokens:` is a positive integer",
        "fallbacks" => "`fallbacks:` is `true` or `false`",
        "base_url" => "`base_url:` is a URL, a string",
        "kind" => "`kind:` is `:chat`, `:embedding` or `:transcription`",
        "dimensions" => "`dimensions:` is a positive integer (the vectors' size)",
        "cache" => "`cache:` is `true`, `false` or `:agents` (the default)",
        "cache_ttl" => "`cache_ttl:` is \"5m\" (the default) or \"1h\"",
        _ => "this option takes another value",
    }
}

/// `price: {input: …, output: …}` (`cache_read:`, `cache_write:` too; no
/// output for an embedding model) or, for a transcription model, `price:
/// {minute: …}`: numbers, none negative.
fn price(value: &Expr, kind: grenat_llm::ModelKind) -> Option<String> {
    use grenat_llm::ModelKind;
    let ExprKind::Hash(pairs) = &value.kind else {
        return Some("`price:` is `{input: …, output: …}`, dollars per million tokens".into());
    };
    let mut entries = Vec::new();
    for (key, value) in pairs {
        let Lit::Text(key) = lit(key) else { return None };
        entries.push((key, lit(value)));
    }
    // a number, none negative; `None` when it cannot be known here
    let amount = |lit: &Lit| match lit {
        Lit::Int(n) => Some(*n >= 0),
        Lit::Float(f) => Some(*f >= 0.0),
        Lit::Computed => None,
        _ => Some(false),
    };
    let get = |name: &str| entries.iter().find(|(k, _)| k == name).map(|(_, v)| v);
    if kind == ModelKind::Transcription && get("minute").is_some() {
        let valid = match entries.as_slice() {
            [(_, minute)] => amount(minute) != Some(false),
            _ => false,
        };
        return (!valid).then(|| {
            "`price:` is `{minute: …}` (dollars per minute of audio, not negative) or `{input: …, output: …}` (per million tokens)"
                .into()
        });
    }
    let required = |name: &str| get(name).is_some_and(|v| amount(v) != Some(false));
    let optional = |name: &str| get(name).is_none_or(|v| amount(v) != Some(false));
    let output = required("output") || (kind == ModelKind::Embedding && get("output").is_none());
    let valid = required("input") && output && optional("cache_read") && optional("cache_write");
    (!valid).then(|| {
        "`price:` is `{input: …, output: …}`, dollars per million tokens, none negative (with `cache_read: …` and `cache_write: …` when the prompt cache is not priced as usual)"
            .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grenat_llm::ModelKind::{Chat, Embedding, Transcription};

    fn expr(src: &str) -> Expr {
        let parsed = grenat_parser::parse(&format!("x = {src}\n"));
        match &parsed.program.items[0] {
            grenat_ast::Item::Stmt(Expr { kind: ExprKind::Assign { value, .. }, .. }) => (**value).clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn literals_are_checked_as_the_interpreter_takes_them() {
        for (name, ok, bad) in [
            ("cache", ["true", ":agents"], ["nil", ":always"]),
            ("cache_ttl", ["\"5m\"", "\"1h\""], [":hour", "3600"]),
            ("dimensions", ["3", "4096"], ["0", "\"3\""]),
            ("kind", [":embedding", "\"embedding\""], [":speech", "1"]),
            ("max_tokens", ["10", "x"], ["0", "1.5"]),
        ] {
            for value in ok {
                assert_eq!(invalid(name, &expr(value), Chat), None, "{name}: {value}");
            }
            for value in bad {
                assert!(invalid(name, &expr(value), Chat).is_some(), "{name}: {value}");
            }
        }
        assert!(invalid("dimensions", &expr("3.5"), Embedding).is_some());
        assert!(invalid("dimensions", &expr("-1"), Embedding).is_some());
    }

    #[test]
    fn prices_are_numbers_none_negative() {
        assert_eq!(invalid("price", &expr("{input: 1, output: 2.5}"), Chat), None);
        assert_eq!(invalid("price", &expr("{input: 1}"), Embedding), None);
        assert_eq!(invalid("price", &expr("{input: rate, output: 2}"), Chat), None);
        assert_eq!(invalid("price", &expr("{minute: 0.006}"), Transcription), None);
        assert!(invalid("price", &expr("{input: 1}"), Chat).is_some());
        assert!(invalid("price", &expr("{input: -1, output: 2}"), Chat).is_some());
        assert!(invalid("price", &expr("{input: 1, output: 2, cache_read: \"x\"}"), Chat).is_some());
        assert!(invalid("price", &expr("{minute: -1}"), Transcription).is_some());
        assert!(invalid("price", &expr("{minute: \"x\"}"), Transcription).is_some());
        assert!(invalid("price", &expr("{minute: 1, input: 2}"), Transcription).is_some());
        assert!(invalid("price", &expr("3"), Chat).is_some());
    }
}
