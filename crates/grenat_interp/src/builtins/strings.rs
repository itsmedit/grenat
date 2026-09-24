//! Methods of `String`.

use crate::prelude::*;

use super::*;

pub(crate) fn str_method<'p>(s: &Arc<str>, name: &str, args: &Args<'p>) -> Option<R<'p>> {
    let text = |t: String| Some(Ok(Value::str(t)));
    let strings = |items: Vec<&str>| Some(Ok(Value::array(items.into_iter().map(Value::str).collect())));
    match name {
        "size" | "length" => Some(Ok(Value::Int(s.chars().count() as i64))),
        "upcase" => text(s.to_uppercase()),
        "downcase" => text(s.to_lowercase()),
        "capitalize" => {
            let mut chars = s.chars();
            text(
                chars
                    .next()
                    .map_or(String::new(), |c| c.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect()),
            )
        }
        "strip" => text(s.trim().to_string()),
        "lstrip" => text(s.trim_start().to_string()),
        "rstrip" => text(s.trim_end().to_string()),
        "reverse" => text(s.chars().rev().collect()),
        "chars" => Some(Ok(Value::array(s.chars().map(|c| Value::str(c.to_string())).collect()))),
        "lines" => strings(s.lines().collect()),
        "split" => match args.pos.first().map(Value::untainted) {
            Some(Value::Str(sep)) => strings(s.split(&**sep).collect()),
            _ => strings(s.split_whitespace().collect()),
        },
        "empty?" => Some(Ok(Value::Bool(s.is_empty()))),
        "include?" => Some(str_arg(args, 0, name).map(|x| Value::Bool(s.contains(&*x)))),
        "start_with?" => Some(str_arg(args, 0, name).map(|x| Value::Bool(s.starts_with(&*x)))),
        "end_with?" => Some(str_arg(args, 0, name).map(|x| Value::Bool(s.ends_with(&*x)))),
        "index" => Some(
            str_arg(args, 0, name)
                .map(|x| s.find(&*x).map_or(Value::Nil, |byte| Value::Int(s[..byte].chars().count() as i64))),
        ),
        "sub" | "gsub" => Some((|| {
            let (from, to) = (str_arg(args, 0, name)?, str_arg(args, 1, name)?);
            Ok(Value::str(if name == "sub" { s.replacen(&*from, &to, 1) } else { s.replace(&*from, &to) }))
        })()),
        "truncate" => Some(int_arg(args, 0, name).map(|n| {
            let n = n.max(1) as usize;
            if s.chars().count() <= n {
                Value::Str(s.clone())
            } else {
                Value::str(format!("{}…", s.chars().take(n - 1).collect::<String>()))
            }
        })),
        "ljust" | "rjust" => Some(int_arg(args, 0, name).map(|n| {
            let pad = " ".repeat((n.max(0) as usize).saturating_sub(s.chars().count()));
            Value::str(if name == "ljust" { format!("{s}{pad}") } else { format!("{pad}{s}") })
        })),
        "to_i" => Some(Ok(Value::Int(s.trim().parse().unwrap_or(0)))),
        "to_f" => Some(Ok(Value::Float(s.trim().parse().unwrap_or(0.0)))),
        "to_sym" => Some(Ok(Value::Symbol(s.clone()))),
        _ => None,
    }
}
