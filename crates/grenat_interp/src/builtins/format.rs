//! `format("%.2f", x)` and `"%05d" % n` (`"%s-%s" % [a, b]`): Ruby's
//! directives `%d` `%i` `%f` `%e` `%s` `%p` `%x` `%X` `%o` `%b` `%%`, with
//! the flags `-` `0` `+` and space, a width and a precision. What is made
//! of an untrusted value is untrusted; of a secret, a secret — and an error
//! never shows a secret's text.

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
    /// A secret's text: written (the result is then a secret), read as a
    /// number, never shown in an error.
    Secret(String),
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
            Value::Secret(s) => Operand::Secret(s.to_string()),
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
            'd' | 'i' | 'u' => self.integer(integer(operand, kind)?, |n| n.to_string()),
            'x' => self.integer(integer(operand, kind)?, |n| format!("{n:x}")),
            'X' => self.integer(integer(operand, kind)?, |n| format!("{n:X}")),
            'o' => self.integer(integer(operand, kind)?, |n| format!("{n:o}")),
            'b' => self.integer(integer(operand, kind)?, |n| format!("{n:b}")),
            'f' | 'e' | 'E' => {
                let f = float(operand, kind)?;
                if !f.is_finite() {
                    return Ok(self.infinite(f));
                }
                let precision = self.precision.unwrap_or(6);
                let text = match kind {
                    'f' => format!("{:.precision$}", f.abs()),
                    'e' => exponent(f.abs(), precision),
                    _ => exponent(f.abs(), precision).to_uppercase(),
                };
                self.number(f.is_sign_negative(), text, true)
            }
            's' | 'p' => {
                let text = match operand {
                    Operand::Int(n) => n.to_string(),
                    Operand::Float(f) => Value::Float(*f).to_display(),
                    Operand::Text { shown, inspected } => (if kind == 's' { shown } else { inspected }).clone(),
                    Operand::Secret(text) => text.clone(),
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

    /// An integer in a base (`digits` of its magnitude): a precision is a
    /// minimum count of digits (`%.3d` of 5 is `005`, `%.0d` of 0 nothing),
    /// and then the `0` flag pads with spaces, as in C and Ruby. A negative
    /// number keeps its sign in any base (`-ff`; Ruby writes `..f01`).
    fn integer(&self, n: i128, digits: impl Fn(u128) -> String) -> String {
        let text = match self.precision {
            Some(0) if n == 0 => String::new(),
            Some(p) => format!("{:0>p$}", digits(n.unsigned_abs())),
            None => digits(n.unsigned_abs()),
        };
        self.number(n < 0, text, self.precision.is_none())
    }

    /// `Inf`, `-Inf` or `NaN`, as Ruby writes them: signed by `+` or a
    /// space, padded with spaces only.
    fn infinite(&self, f: f64) -> String {
        let text = if f.is_nan() { "NaN" } else { "Inf" };
        self.number(f.is_sign_negative() && !f.is_nan(), text.to_string(), false)
    }

    /// A number's sign, then its `digits`, padded (with zeros after the
    /// sign when `zeros` may and the `0` flag asks).
    fn number(&self, negative: bool, digits: String, zeros: bool) -> String {
        let sign = match () {
            _ if negative => "-",
            _ if self.plus => "+",
            _ if self.space => " ",
            _ => "",
        };
        if zeros && self.zero && !self.left {
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

/// An integer operand: a float truncated toward zero, as Ruby does (`%d` of
/// -3.99 is -3), exact past an `i64` (`%d` of 1e20).
fn integer(operand: &Operand, kind: char) -> Result<i128, String> {
    match operand {
        Operand::Int(n) => Ok(i128::from(*n)),
        Operand::Float(f) if !f.is_finite() => Err(format!("`%{kind}` expects an integer, got {}", ruby_float(*f))),
        // 2^127: past it, no i128 holds the number
        Operand::Float(f) if f.trunc().abs() >= 1.7014118346046923e38 => {
            Err(format!("`%{kind}`: {f:e} is out of range"))
        }
        Operand::Float(f) => Ok(f.trunc() as i128),
        Operand::Text { shown, .. } => {
            shown.trim().parse().map_err(|_| format!("`%{kind}` expects an integer, got {shown:?}"))
        }
        Operand::Secret(text) => text.trim().parse().map_err(|_| unreadable_secret(kind, "an integer")),
    }
}

fn float(operand: &Operand, kind: char) -> Result<f64, String> {
    match operand {
        Operand::Int(n) => Ok(*n as f64),
        Operand::Float(f) => Ok(*f),
        Operand::Text { shown, .. } => {
            shown.trim().parse().map_err(|_| format!("`%{kind}` expects a number, got {shown:?}"))
        }
        Operand::Secret(text) => text.trim().parse().map_err(|_| unreadable_secret(kind, "a number")),
    }
}

/// Why a secret is no number, its text never in it.
fn unreadable_secret(kind: char, expected: &str) -> String {
    format!("`%{kind}` expects {expected}, got a secret that is not one (its text is not shown)")
}

/// A float as Ruby names the ones that are not finite: `Inf`, `-Inf`, `NaN`.
fn ruby_float(f: f64) -> String {
    match f {
        f if f.is_nan() => "NaN".into(),
        f if f.is_infinite() => (if f < 0.0 { "-Inf" } else { "Inf" }).into(),
        f => Value::Float(f).to_display(),
    }
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
    fn integers_as_ruby_writes_them() {
        let f = |spec: &str, x: Operand| format(spec, &[x]).unwrap();
        // a float is truncated toward zero, never floored
        assert_eq!(f("%d", Operand::Float(-3.99)), "-3");
        assert_eq!(f("%d", Operand::Float(-0.5)), "0");
        // past an i64, still exact
        assert_eq!(f("%d", Operand::Float(1e20)), "100000000000000000000");
        assert_eq!(f("%x", Operand::Float(1e20)), "56bc75e2d63100000");
        // a precision is a minimum count of digits; the `0` flag then pads with spaces
        assert_eq!(f("%.3d", Operand::Int(5)), "005");
        assert_eq!(f("%.3d", Operand::Int(-5)), "-005");
        assert_eq!(f("%+.3d", Operand::Int(5)), "+005");
        assert_eq!(f("%05.3d", Operand::Int(5)), "  005");
        assert_eq!(f("%-6.3d|", Operand::Int(5)), "005   |");
        assert_eq!(f("%.3x", Operand::Int(5)), "005");
        assert_eq!(f("%.3b", Operand::Int(1)), "001");
        assert_eq!(f("%.0d", Operand::Int(0)), "");
        assert_eq!(f("%5.0d|", Operand::Int(0)), "     |");
        assert_eq!(f("%x", Operand::Float(255.9)), "ff");
        // a negative number keeps its sign in another base (Ruby writes `..f01`)
        assert_eq!(f("%x", Operand::Int(-255)), "-ff");
    }

    #[test]
    fn infinities_and_nan_as_ruby_writes_them() {
        let f = |spec: &str, x: f64| format(spec, &[Operand::Float(x)]).unwrap();
        assert_eq!(f("%f", f64::INFINITY), "Inf");
        assert_eq!(f("%f", f64::NEG_INFINITY), "-Inf");
        assert_eq!(f("%f", f64::NAN), "NaN");
        assert_eq!(f("%010f", f64::INFINITY), "       Inf");
        assert_eq!(f("%+f", f64::INFINITY), "+Inf");
        assert_eq!(f("% f", f64::INFINITY), " Inf");
        assert_eq!(f("%-6f|", f64::NAN), "NaN   |");
        assert_eq!(f("%e", f64::INFINITY), "Inf");
        assert_eq!(f("%.2E", f64::NEG_INFINITY), "-Inf");
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
        assert_eq!(format("%d", &[Operand::Float(f64::INFINITY)]).unwrap_err(), "`%d` expects an integer, got Inf");
        assert_eq!(format("%x", &[Operand::Float(f64::NAN)]).unwrap_err(), "`%x` expects an integer, got NaN");
        assert_eq!(format("%d", &[Operand::Float(1e40)]).unwrap_err(), "`%d`: 1e40 is out of range");
        assert!(format("50%", &[]).unwrap_err().contains("incomplete"));
        for spec in ["%d", "%x", "%f", "%e"] {
            let error = format(spec, &[Operand::Secret("sk-123".into())]).unwrap_err();
            assert!(error.contains("got a secret") && !error.contains("sk-123"), "{spec}: {error}");
        }
        assert_eq!(format("%03d|%s", &[Operand::Secret("7".into()), Operand::Secret("t".into())]).unwrap(), "007|t");
    }
}
