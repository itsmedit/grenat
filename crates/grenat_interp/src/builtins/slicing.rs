//! Slices, as in Ruby: `s[start, length]`, `s[a..b]`, `s[a...b]` (strings
//! by characters, never bytes), `xs[start, length]`, `xs[a..b]`, and their
//! `slice` methods. A negative index counts from the end; a start past the
//! end is `nil`, a start at the end is empty, a length or an end past the
//! end stops there.

use crate::prelude::*;

/// The bounds `[from, to)` of `start, length` in a sequence of `len` items,
/// or `None` when Ruby gives `nil`.
pub(crate) fn start_length(len: usize, start: i64, length: i64) -> Option<(usize, usize)> {
    let from = start_of(len, start)?;
    if length < 0 {
        return None;
    }
    Some((from, from.saturating_add(length as usize).min(len)))
}

/// The bounds `[from, to)` of the range `lo..hi` (`lo...hi`) in a sequence
/// of `len` items, or `None` when Ruby gives `nil`.
pub(crate) fn range(len: usize, lo: i64, hi: i64, inclusive: bool) -> Option<(usize, usize)> {
    let from = start_of(len, lo)?;
    let hi = if hi < 0 { hi + len as i64 } else { hi };
    let end = (hi + i64::from(inclusive)).clamp(from as i64, len as i64);
    Some((from, end as usize))
}

/// A start counted from the end when negative; `None` outside `0..=len`.
fn start_of(len: usize, start: i64) -> Option<usize> {
    let start = if start < 0 { start + len as i64 } else { start };
    (0..=len as i64).contains(&start).then_some(start as usize)
}

/// What selects a slice: `(start, length)` or a range; `None` for a single
/// index (or anything else).
pub(crate) fn bounds<'p>(index: &[Value<'p>], len: usize) -> Result<Option<Option<(usize, usize)>>, Ctrl<'p>> {
    match index {
        [start, length] => match (start.untainted(), length.untainted()) {
            (Value::Int(start), Value::Int(length)) => Ok(Some(start_length(len, *start, *length))),
            (start, length) => raise(
                "TypeError",
                format!(
                    "a slice `[start, length]` takes two integers, got {} and {}",
                    start.type_name(),
                    length.type_name()
                ),
            ),
        },
        [single] => Ok(match single.untainted() {
            Value::Range(lo, hi, inclusive) => Some(range(len, *lo, *hi, *inclusive)),
            _ => None,
        }),
        _ => raise("ArgumentError", format!("an index takes one or two values, got {}", index.len())),
    }
}

/// The slice of a string, by characters.
pub(crate) fn of_str<'p>(s: &str, bounds: Option<(usize, usize)>) -> Value<'p> {
    match bounds {
        Some((from, to)) => Value::str(s.chars().skip(from).take(to - from).collect::<String>()),
        None => Value::Nil,
    }
}

/// The slice of an array: a new array.
pub(crate) fn of_array<'p>(items: &[Value<'p>], bounds: Option<(usize, usize)>) -> Value<'p> {
    match bounds {
        Some((from, to)) => Value::array(items[from..to].to_vec()),
        None => Value::Nil,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_and_length() {
        assert_eq!(start_length(6, 0, 4), Some((0, 4)));
        assert_eq!(start_length(6, 2, 100), Some((2, 6)));
        assert_eq!(start_length(6, -3, 2), Some((3, 5)));
        assert_eq!(start_length(6, 6, 2), Some((6, 6)));
        assert_eq!(start_length(6, 7, 1), None);
        assert_eq!(start_length(6, -7, 1), None);
        assert_eq!(start_length(6, 0, -1), None);
        assert_eq!(start_length(0, 0, 3), Some((0, 0)));
    }

    #[test]
    fn ranges() {
        assert_eq!(range(6, 0, 3, true), Some((0, 4)));
        assert_eq!(range(6, 0, 3, false), Some((0, 3)));
        assert_eq!(range(6, 1, -1, true), Some((1, 6)));
        assert_eq!(range(6, 1, -1, false), Some((1, 5)));
        assert_eq!(range(6, -2, -1, true), Some((4, 6)));
        assert_eq!(range(6, 4, 1, true), Some((4, 4)));
        assert_eq!(range(6, 2, 100, true), Some((2, 6)));
        assert_eq!(range(6, 6, 9, true), Some((6, 6)));
        assert_eq!(range(6, 7, 9, true), None);
        assert_eq!(range(6, -7, 2, true), None);
    }

    #[test]
    fn strings_are_sliced_by_characters() {
        let s = "héllo wörld";
        assert_eq!(of_str(s, start_length(11, 0, 5)).to_display(), "héllo");
        assert_eq!(of_str(s, range(11, 6, -1, true)).to_display(), "wörld");
        assert!(matches!(of_str(s, start_length(11, 12, 1)), Value::Nil));
    }
}
