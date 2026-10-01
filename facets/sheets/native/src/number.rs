//! When a text is a number, for typed writing: exactly when the number,
//! read back from the sheet, gives the same text (see [`crate::cells`]).
//!
//! `42`, `-3.5` and `0.25` are numbers; `007` (a code), `1.50`, `+1`,
//! `1e3`, ` 1`, `1,5`, `NaN` and `12345678901234567890` (more digits than a
//! number keeps) stay text: writing them as numbers would change them.

use crate::cells;

/// The number `text` stands for, if writing it as one keeps it intact.
pub fn number(text: &str) -> Option<f64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (digits, None),
    };
    let decimal = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !decimal(whole) || !fraction.is_none_or(decimal) {
        return None;
    }
    let value: f64 = text.parse().ok()?;
    (value.is_finite() && cells::float(value) == text).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_that_read_back_the_same() {
        assert_eq!(number("42"), Some(42.0));
        assert_eq!(number("-3.5"), Some(-3.5));
        assert_eq!(number("0.25"), Some(0.25));
        assert_eq!(number("0"), Some(0.0));
        assert_eq!(number("1048576"), Some(1_048_576.0));
    }

    #[test]
    fn texts_that_would_change_stay_text() {
        for text in [
            "",
            "-",
            "007",
            "1.50",
            "1.",
            ".5",
            "+1",
            "1e3",
            " 1",
            "1 ",
            "1,5",
            "NaN",
            "inf",
            "-0",
            "0x10",
            "١٢",
            "12345678901234567890",
            "abc",
        ] {
            assert_eq!(number(text), None, "{text:?}");
        }
    }
}
