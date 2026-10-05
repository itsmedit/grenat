//! `format("%.2f", x)` and `"%05d" % n` (`"%s-%s" % [a, b]`): Ruby's
//! directives `%d` `%i` `%f` `%e` `%s` `%p` `%x` `%X` `%o` `%b` `%%`, with
//! the flags `-` `0` `+` and space, a width and a precision. What is made
//! of an untrusted value is untrusted; of a secret, a secret.

use crate::prelude::*;

/// A value to write, as the directives see it.
pub(crate) enum Operand {
    Int(i64),
    Float(f64),
    /// Text: how it is shown (`%s`) and inspected (`%p`).
    Text {
        shown: String,
        inspected: String,
    },
}

/// `format(spec, values…)` on interpreter values.
pub(crate) fn format_values<'p>(interp: &mut Interp<'p>, spec: &Value<'p>, values: &[Value<'p>]) -> R<'p> {
    let spec_text = match spec.untainted() {
        Value::Str(s) => s.clone(),
        other => return raise("TypeError", format!("`format` expects a format string, got {}", other.type_name())),
    };
    let mut operands = Vec::with_capacity(values.len());
    for v in values {
        operands.push(match v.untainted() {
            Value::Int(n) => Operand::Int(*n),
            Value::Float(f) | Value::Money(f) | Value::Duration(f) => Operand::Float(*f),
            Value::Secret(s) => Operand::Text { shown: s.to_string(), inspected: s.to_string() },
            other => Operand::Text { shown: interp.display(other)?, inspected: other.inspect() },
        });
    }
    let text = format(&spec_text, &operands).or_else(|e| raise("ArgumentError", e))?;
    let value = if values.iter().any(Value::contains_secret) { Value::Secret(text.into()) } else { Value::str(text) };
    let untrusted = spec.contains_taint() || values.iter().any(Value::contains_taint);
    Ok(if untrusted { value.taint() } else { value })
}

/// The text of `spec` with its directives replaced by `operands`.
pub(crate) fn format(spec: &str, operands: &[Operand]) -> Result<String, String> {
    let mut out = String::with_capacity(spec.len());
    let mut chars = spec.chars().peekable();
    let mut next = operands.iter();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut d = Directive::default();
        while let Some(&flag) = chars.peek() {
            match flag {
                '-' => d.left = true,
                '0' => d.zero = true,
                '+' => d.plus = true,
                ' ' => d.space = true,
                _ => break,
            }
            chars.next();
        }
        d.width = digits(&mut chars);
        if chars.peek() == Some(&'.') {
            chars.next();
            d.precision = Some(digits(&mut chars).unwrap_or(0));
        }
        let Some(kind) = chars.next() else { return Err("incomplete format specifier: use `%%` for a `%`".into()) };
        if kind == '%' {
            out.push('%');
            continue;
        }
        let operand = next.next().ok_or("too few arguments for the format")?;
        out.push_str(&d.write(kind, operand)?);
    }
    Ok(out)
}

fn digits(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<usize> {
    let mut n: Option<usize> = None;
    while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
        n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(d as usize));
        chars.next();
    }
    n
}

#[derive(Default)]
struct Directive {
    left: bool,
    zero: bool,
    plus: bool,
    space: bool,
    width: Option<usize>,
    precision: Option<usize>,
}

impl Directive {
    fn write(&self, kind: char, operand: &Operand) -> Result<String, String> {
        Ok(match kind {
            'd' | 'i' | 'u' => self.number(integer(operand, kind)?.to_string()),
            'x' => self.number(radix(integer(operand, kind)?, |n| format!("{n:x}"))),
            'X' => self.number(radix(integer(operand, kind)?, |n| format!("{n:X}"))),
            'o' => self.number(radix(integer(operand, kind)?, |n| format!("{n:o}"))),
            'b' => self.number(radix(integer(operand, kind)?, |n| format!("{n:b}"))),
            'f' => self.number(format!("{:.*}", self.precision.unwrap_or(6), float(operand, kind)?)),
            'e' | 'E' => {
                let text = exponent(float(operand, kind)?, self.precision.unwrap_or(6));
                self.number(if kind == 'E' { text.to_uppercase() } else { text })
            }
            's' | 'p' => {
                let text = match operand {
                    Operand::Int(n) => n.to_string(),
                    Operand::Float(f) => Value::Float(*f).to_display(),
                    Operand::Text { shown, inspected } => (if kind == 's' { shown } else { inspected }).clone(),
                };
                let text = match self.precision {
                    Some(p) => text.chars().take(p).collect(),
                    None => text,
                };
                self.pad(text)
            }
            other => return Err(format!("unknown format directive `%{other}`")),
        })
    }

    /// A number: its sign, then padding (zeros after the sign).
    fn number(&self, text: String) -> String {
        let (sign, digits) = match text.strip_prefix('-') {
            Some(rest) => ("-", rest.to_string()),
            None if self.plus => ("+", text),
            None if self.space => (" ", text),
            None => ("", text),
        };
        if self.zero && !self.left {
            let width = self.width.unwrap_or(0).saturating_sub(sign.len());
            return format!("{sign}{digits:0>width$}");
        }
        self.pad(format!("{sign}{digits}"))
    }

    /// Spaces up to the width, on the left unless `-`.
    fn pad(&self, text: String) -> String {
        let missing = self.width.unwrap_or(0).saturating_sub(text.chars().count());
        let padding = " ".repeat(missing);
        if self.left { format!("{text}{padding}") } else { format!("{padding}{text}") }
    }
}

fn integer(operand: &Operand, kind: char) -> Result<i64, String> {
    match operand {
        Operand::Int(n) => Ok(*n),
        Operand::Float(f) if f.is_finite() => Ok(f.floor() as i64),
        Operand::Text { shown, .. } => {
            shown.trim().parse().map_err(|_| format!("`%{kind}` expects an integer, got {shown:?}"))
        }
        Operand::Float(f) => Err(format!("`%{kind}` expects an integer, got {f}")),
    }
}

fn float(operand: &Operand, kind: char) -> Result<f64, String> {
    match operand {
        Operand::Int(n) => Ok(*n as f64),
        Operand::Float(f) => Ok(*f),
        Operand::Text { shown, .. } => {
            shown.trim().parse().map_err(|_| format!("`%{kind}` expects a number, got {shown:?}"))
        }
    }
}

/// A negative number in another base keeps its sign: `-ff`.
fn radix(n: i64, digits: impl Fn(u64) -> String) -> String {
    if n < 0 { format!("-{}", digits(n.unsigned_abs())) } else { digits(n as u64) }
}

/// `1.234500e+03`: as C and Ruby write it, two exponent digits at least.
fn exponent(f: f64, precision: usize) -> String {
    let text = format!("{f:.precision$e}");
    match text.split_once('e') {
        Some((mantissa, exp)) => {
            let (sign, digits) = match exp.strip_prefix('-') {
                Some(d) => ('-', d),
                None => ('+', exp),
            };
            format!("{mantissa}e{sign}{digits:0>2}")
        }
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Operand {
        Operand::Text { shown: s.into(), inspected: format!("{s:?}") }
    }

    #[test]
    fn numbers() {
        assert_eq!(format("%.2f", &[Operand::Float(12.3456)]).unwrap(), "12.35");
        assert_eq!(format("%d items", &[Operand::Int(42)]).unwrap(), "42 items");
        assert_eq!(format("%05d", &[Operand::Int(42)]).unwrap(), "00042");
        assert_eq!(format("%05d", &[Operand::Int(-42)]).unwrap(), "-0042");
        assert_eq!(format("%+d %+d", &[Operand::Int(5), Operand::Int(-5)]).unwrap(), "+5 -5");
        assert_eq!(
            format("%x %X %o %b", &[Operand::Int(255), Operand::Int(255), Operand::Int(8), Operand::Int(5)]).unwrap(),
            "ff FF 10 101"
        );
        assert_eq!(format("%d", &[Operand::Float(3.99)]).unwrap(), "3");
        assert_eq!(format("%.1f", &[Operand::Int(2)]).unwrap(), "2.0");
        assert_eq!(format("%f", &[Operand::Float(1.5)]).unwrap(), "1.500000");
        assert_eq!(format("%.2e", &[Operand::Float(1234.5)]).unwrap(), "1.23e+03");
        assert_eq!(format("%8.2f|", &[Operand::Float(12.3456)]).unwrap(), "   12.35|");
        assert_eq!(format("%-8.2f|", &[Operand::Float(12.3456)]).unwrap(), "12.35   |");
    }

    #[test]
    fn text_and_percent() {
        assert_eq!(format("%s-%s", &[text("a"), Operand::Int(1)]).unwrap(), "a-1");
        assert_eq!(format("%-5s|%5s|", &[text("ab"), text("cd")]).unwrap(), "ab   |   cd|");
        assert_eq!(format("%.3s", &[text("abcdef")]).unwrap(), "abc");
        assert_eq!(format("%p", &[text("a")]).unwrap(), "\"a\"");
        assert_eq!(format("100%%", &[]).unwrap(), "100%");
        assert_eq!(format("%d", &[text("12")]).unwrap(), "12");
    }

    #[test]
    fn refusals() {
        assert!(format("%d %d", &[Operand::Int(1)]).unwrap_err().contains("too few"));
        assert!(format("%q", &[Operand::Int(1)]).unwrap_err().contains("`%q`"));
        assert!(format("%d", &[text("abc")]).unwrap_err().contains("expects an integer"));
        assert!(format("50%", &[]).unwrap_err().contains("incomplete"));
    }
}
