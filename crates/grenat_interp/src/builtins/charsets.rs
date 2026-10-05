//! Ruby's character sets, for `String#count`, `delete`, `squeeze` and
//! `tr`: `"aeiou"` lists characters, `"a-z"` is a range, a leading `^`
//! negates (`"^a-z"`), `\` escapes `-`, `^` and itself. Several sets
//! intersect (`count("a-y", "^l")`: in both). No regular expressions:
//! Grenat has none.

/// A set of characters, maybe negated.
pub(crate) struct CharSet {
    chars: Vec<char>,
    negated: bool,
}

impl CharSet {
    pub(crate) fn parse(spec: &str) -> CharSet {
        let negated = spec.starts_with('^') && spec.chars().count() > 1;
        let body = if negated { &spec[1..] } else { spec };
        CharSet { chars: expand(body), negated }
    }

    pub(crate) fn contains(&self, c: char) -> bool {
        self.chars.contains(&c) != self.negated
    }
}

/// Whether every set holds `c`: several sets intersect, as in Ruby.
pub(crate) fn in_all(sets: &[CharSet], c: char) -> bool {
    sets.iter().all(|set| set.contains(c))
}

/// The characters of a set's text, ranges expanded (`"a-c"` → `a b c`); a
/// `-` first or last, or a descending range, stands for itself.
fn expand(spec: &str) -> Vec<char> {
    let mut items: Vec<(char, bool)> = Vec::new();
    let mut chars = spec.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => items.push((chars.next().unwrap_or('\\'), true)),
            c => items.push((c, false)),
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let (c, _) = items[i];
        if let (Some(('-', false)), Some((end, _))) = (items.get(i + 1), items.get(i + 2))
            && c <= *end
        {
            out.extend(c..=*end);
            i += 3;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// `tr(from, to)`: each character of `from` becomes the one at the same
/// place in `to` (the last one of `to` when it is shorter); with `^from`,
/// every character outside it becomes the last of `to`. An empty `to`
/// deletes.
pub(crate) fn translate(s: &str, from: &str, to: &str) -> String {
    let negated = from.starts_with('^') && from.chars().count() > 1;
    let from = expand(if negated { &from[1..] } else { from });
    let to = expand(to);
    let Some(&last) = to.last() else {
        let set = CharSet { chars: from, negated };
        return s.chars().filter(|c| !set.contains(*c)).collect();
    };
    s.chars()
        .map(|c| match (negated, from.iter().position(|f| *f == c)) {
            (true, None) => last,
            (true, Some(_)) | (false, None) => c,
            (false, Some(i)) => to.get(i).copied().unwrap_or(last),
        })
        .collect()
}

/// `squeeze`: runs of the same character (of every set, when given)
/// become one.
pub(crate) fn squeeze(s: &str, sets: &[CharSet]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut previous = None;
    for c in s.chars() {
        if previous == Some(c) && in_all(sets, c) {
            continue;
        }
        out.push(c);
        previous = Some(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_list_ranges_and_negate() {
        let vowels = CharSet::parse("aeiou");
        assert!(vowels.contains('e') && !vowels.contains('x'));
        let lower = CharSet::parse("a-z");
        assert!(lower.contains('m') && !lower.contains('M') && !lower.contains('-'));
        let not_digits = CharSet::parse("^0-9");
        assert!(not_digits.contains('a') && !not_digits.contains('5'));
        assert!(CharSet::parse("^").contains('^'));
        assert!(CharSet::parse("a-").contains('-'));
        assert!(CharSet::parse("a\\-z").contains('-') && !CharSet::parse("a\\-z").contains('b'));
    }

    #[test]
    fn translation() {
        assert_eq!(translate("hello", "el", "ip"), "hippo");
        assert_eq!(translate("hello", "a-y", "b-z"), "ifmmp");
        assert_eq!(translate("hello", "a-z", "A-Z"), "HELLO");
        assert_eq!(translate("hello", "lo", "x"), "hexxx");
        assert_eq!(translate("hello", "^l", "*"), "**ll*");
        assert_eq!(translate("hello", "l", ""), "heo");
        assert_eq!(translate("héllo", "é", "e"), "hello");
    }

    #[test]
    fn squeezing() {
        assert_eq!(squeeze("aaabbbccc", &[]), "abc");
        assert_eq!(squeeze("a  b   c", &[CharSet::parse(" ")]), "a b c");
        assert_eq!(squeeze("mississippi", &[CharSet::parse("s")]), "misisippi");
        assert_eq!(squeeze("aaabbboo", &[CharSet::parse("ab"), CharSet::parse("b")]), "aaaboo");
    }

    #[test]
    fn several_sets_are_intersected() {
        let sets = [CharSet::parse("lo"), CharSet::parse("o")];
        assert!(in_all(&sets, 'o') && !in_all(&sets, 'l'));
        let sets = [CharSet::parse("a-y"), CharSet::parse("^l")];
        assert!(in_all(&sets, 'h') && !in_all(&sets, 'l') && !in_all(&sets, 'z'));
        // an empty set holds nothing
        assert!(!in_all(&[CharSet::parse("lo"), CharSet::parse("")], 'l'));
    }
}
