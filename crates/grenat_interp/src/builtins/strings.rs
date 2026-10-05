//! Methods of `String`. Text made with an untrusted argument (a
//! replacement, a padding) is untrusted.

use crate::prelude::*;

use super::charsets::{self, CharSet};
use super::*;

/// The methods whose text is made of their arguments too.
const EMBEDS_ARGS: &[&str] = &["sub", "gsub", "ljust", "rjust", "center", "tr"];

pub(crate) fn str_method<'p>(s: &Arc<str>, name: &str, args: &Args<'p>) -> Option<R<'p>> {
    let result = text_method(s, name, args)?;
    let untrusted = EMBEDS_ARGS.contains(&name) && args.pos.iter().any(Value::contains_taint);
    Some(result.map(|v| if untrusted { v.taint() } else { v }))
}

fn text_method<'p>(s: &Arc<str>, name: &str, args: &Args<'p>) -> Option<R<'p>> {
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
        "start_with?" | "end_with?" => Some((|| {
            let mut found = false;
            for i in 0..args.pos.len().max(1) {
                let x = str_arg(args, i, name)?;
                found |= if name == "start_with?" { s.starts_with(&*x) } else { s.ends_with(&*x) };
            }
            Ok(Value::Bool(found))
        })()),
        "delete_prefix" => Some(str_arg(args, 0, name).map(|x| Value::str(s.strip_prefix(&*x).unwrap_or(s)))),
        "delete_suffix" => Some(str_arg(args, 0, name).map(|x| Value::str(s.strip_suffix(&*x).unwrap_or(s)))),
        "slice" => Some((|| {
            let len = s.chars().count();
            match super::slicing::bounds(&args.pos, len)? {
                Some(bounds) => Ok(super::slicing::of_str(s, bounds)),
                None => {
                    let i = int_arg(args, 0, name)?;
                    Ok(super::slicing::of_str(s, super::slicing::start_length(len, i, 1).filter(|(a, b)| a < b)))
                }
            }
        })()),
        "swapcase" => text(
            s.chars()
                .flat_map(|c| -> Box<dyn Iterator<Item = char>> {
                    if c.is_uppercase() { Box::new(c.to_lowercase()) } else { Box::new(c.to_uppercase()) }
                })
                .collect(),
        ),
        "count" => Some(
            char_sets(args, name, 1)
                .map(|sets| Value::Int(s.chars().filter(|c| charsets::in_all(&sets, *c)).count() as i64)),
        ),
        "delete" => Some(
            char_sets(args, name, 1)
                .map(|sets| Value::str(s.chars().filter(|c| !charsets::in_all(&sets, *c)).collect::<String>())),
        ),
        "squeeze" => Some(char_sets(args, name, 0).map(|sets| Value::str(charsets::squeeze(s, &sets)))),
        "tr" => Some((|| {
            let (from, to) = (str_arg(args, 0, name)?, str_arg(args, 1, name)?);
            Ok(Value::str(charsets::translate(s, &from, &to)))
        })()),
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
        "ljust" | "rjust" | "center" => Some((|| {
            let width = int_arg(args, 0, name)?;
            let fill = match args.pos.get(1) {
                Some(_) => str_arg(args, 1, name)?,
                None => " ".into(),
            };
            if fill.is_empty() {
                return raise("ArgumentError", format!("`{name}`: the padding is empty"));
            }
            let missing = (width.max(0) as usize).saturating_sub(s.chars().count());
            let (left, right) = match name {
                "ljust" => (0, missing),
                "rjust" => (missing, 0),
                _ => (missing / 2, missing - missing / 2),
            };
            Ok(Value::str(format!("{}{s}{}", padding(&fill, left), padding(&fill, right))))
        })()),
        "to_i" => Some(Ok(Value::Int(s.trim().parse().unwrap_or(0)))),
        "to_f" => Some(Ok(Value::Float(s.trim().parse().unwrap_or(0.0)))),
        "to_sym" => Some(Ok(Value::Symbol(s.clone()))),
        _ => None,
    }
}

/// The character sets given (`at_least` of them), to intersect.
fn char_sets<'p>(args: &Args<'p>, name: &str, at_least: usize) -> Result<Vec<CharSet>, Ctrl<'p>> {
    (0..args.pos.len().max(at_least)).map(|i| str_arg(args, i, name).map(|set| CharSet::parse(&set))).collect()
}

/// `n` characters of `fill` repeated, as Ruby pads.
fn padding(fill: &str, n: usize) -> String {
    fill.chars().cycle().take(n).collect()
}
