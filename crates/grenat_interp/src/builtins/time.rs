//! The `Time` module. An instant is a `Float` of seconds since the epoch;
//! `Time.now` and `Time.today` read the clock (a `time` effect, stopped in a
//! test by `freeze_time`), the others compute in UTC: `Time.parse(text)`,
//! `Time.iso(t)`, `Time.date(t)`, `Time.weekday(t)`, `Time.at(y, m, d, …)`.
//! What they make of an untrusted value is untrusted.

use crate::prelude::*;

use super::iso8601;
use super::*;

pub(crate) fn call_time<'p>(interp: &mut Interp<'p>, name: &str, args: Args<'p>) -> R<'p> {
    let untrusted = args.pos.iter().any(Value::contains_taint);
    let value = match name {
        "now" => Value::Float(interp.now()),
        "today" => Value::str(iso8601::format_date(interp.now()).or_else(|e| raise("ArgumentError", e))?),
        "parse" => {
            let text = match arg(&args, 0, "Time.parse")?.untainted() {
                Value::Str(s) => s.clone(),
                other => {
                    return raise("TypeError", format!("`Time.parse` expects a string, got {}", other.type_name()));
                }
            };
            Value::Float(iso8601::parse(&text).or_else(|e| raise("ArgumentError", e))?)
        }
        "iso" | "date" | "weekday" => {
            let t = instant(&args, name)?;
            let made = match name {
                "iso" => iso8601::format(t).map(Value::str),
                "date" => iso8601::format_date(t).map(Value::str),
                _ => iso8601::civil_of(t).map(|c| Value::Int(i64::from(c.weekday))),
            };
            made.or_else(|e| raise("ArgumentError", format!("`Time.{name}`: {e}")))?
        }
        "at" => Value::Float(at(&args)?),
        _ => return raise("NoMethodError", format!("unknown method `Time.{name}`")),
    };
    Ok(if untrusted { value.taint() } else { value })
}

/// The instant argument: epoch seconds, a `Float` or an `Int`.
fn instant<'p>(args: &Args<'p>, name: &str) -> Result<f64, Ctrl<'p>> {
    match arg(args, 0, &format!("Time.{name}"))?.untainted() {
        Value::Float(t) => Ok(*t),
        Value::Int(n) => Ok(*n as f64),
        other => raise(
            "TypeError",
            format!("`Time.{name}` expects an instant (epoch seconds, `Time.now`), got {}", other.type_name()),
        ),
    }
}

/// `Time.at(year, month, day, hour = 0, min = 0, sec = 0)`, UTC.
fn at<'p>(args: &Args<'p>) -> Result<f64, Ctrl<'p>> {
    const USAGE: &str = "`Time.at` expects `Time.at(year, month, day, hour = 0, min = 0, sec = 0)`";
    if !(3..=6).contains(&args.pos.len()) || !args.named.is_empty() {
        return raise("ArgumentError", USAGE);
    }
    let mut parts = [0i64; 5];
    for (i, part) in parts.iter_mut().enumerate() {
        *part = match args.pos.get(i).map(Value::untainted) {
            Some(Value::Int(n)) => *n,
            None => 0,
            Some(other) => return raise("TypeError", format!("{USAGE}: integers, got {}", other.type_name())),
        };
    }
    let second = match args.pos.get(5).map(Value::untainted) {
        Some(Value::Int(n)) => *n as f64,
        Some(Value::Float(f)) => *f,
        None => 0.0,
        Some(other) => return raise("TypeError", format!("{USAGE}: seconds are a number, got {}", other.type_name())),
    };
    let [year, month, day, hour, minute] = parts;
    iso8601::seconds_of(year, month, day, hour, minute, second)
        .or_else(|e| raise("ArgumentError", format!("`Time.at`: {e}")))
}
