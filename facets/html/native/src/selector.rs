//! Parsing a CSS selector: a selector that does not parse is an error
//! message naming it, never a panic.
//!
//! The selector may come from a user or a model, and the parser and the
//! matcher recurse into it: a selector nesting `:not(` a hundred thousand
//! times would overflow the stack, which kills the process — no error can
//! be raised from that. So a selector longer than `MAX_LENGTH` bytes, or
//! nesting parentheses deeper than `MAX_NESTING`, is refused before it is
//! parsed; no selector written for a page comes near either.

use scraper::Selector;
use scraper::error::SelectorErrorKind;

/// The longest selector parsed, in bytes.
pub const MAX_LENGTH: usize = 4096;

/// How deep a selector's parentheses (`:not(`, `:is(`, `:has(`…) may nest.
pub const MAX_NESTING: usize = 32;

/// How much of a refused selector its error quotes, in characters.
const QUOTED: usize = 40;

/// The selector `text`, parsed, or why it is not one.
pub fn parse(text: &str) -> Result<Selector, String> {
    if text.trim().is_empty() {
        return Err("invalid CSS selector ``: it is empty".to_string());
    }
    if text.len() > MAX_LENGTH {
        return Err(format!("invalid CSS selector `{}`: it is too long (more than {MAX_LENGTH} bytes)", excerpt(text)));
    }
    if nesting(text) > MAX_NESTING {
        return Err(format!(
            "invalid CSS selector `{}`: it nests too deeply (more than {MAX_NESTING} parentheses inside one another)",
            excerpt(text)
        ));
    }
    Selector::parse(text).map_err(|e| format!("invalid CSS selector `{text}`: {}", reason(&e)))
}

/// The start of a selector too big to quote whole.
fn excerpt(text: &str) -> String {
    match text.char_indices().nth(QUOTED) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// How deep the parentheses of `text` nest, those in quoted strings and
/// escaped ones (`\(`) aside.
fn nesting(text: &str) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', _) => {
                chars.next();
            }
            (c, Some(q)) if c == q => quote = None,
            (_, Some(_)) => {}
            ('"' | '\'', None) => quote = Some(c),
            ('(', None) => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            (')', None) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// What is wrong with a selector, in words that start in lowercase.
fn reason(error: &SelectorErrorKind<'_>) -> String {
    match error {
        SelectorErrorKind::EndOfLine => "it ends too early".to_string(),
        // the library's own message for this one is a debugging dump: the
        // name of its kind says it (`DanglingCombinator`: "dangling combinator")
        SelectorErrorKind::UnexpectedSelectorParseError(kind) => {
            let words = words(&format!("{kind:?}"));
            if words.is_empty() { "it is malformed".to_string() } else { words }
        }
        other => lowercase_first(&other.to_string()),
    }
}

/// `DanglingCombinator(…)` → `dangling combinator`: the leading name of a
/// debug representation, as lowercase words.
fn words(debug: &str) -> String {
    let name: String = debug.chars().take_while(char::is_ascii_alphanumeric).collect();
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() && !out.is_empty() {
            out.push(' ');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn lowercase_first(message: &str) -> String {
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => "it is malformed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_selector_parses() {
        for text in ["p", "a[href]", "ul > li.item:nth-child(2)", "#main .title, h1", "input[type=\"text\"]"] {
            assert!(parse(text).is_ok(), "{text}");
        }
    }

    #[test]
    fn an_invalid_selector_is_an_error_naming_it() {
        for text in ["a[", "p >", "::", "div..x", "[[]", "a:not("] {
            let e = parse(text).unwrap_err();
            assert!(e.starts_with(&format!("invalid CSS selector `{text}`: ")), "{e}");
            let reason = &e[format!("invalid CSS selector `{text}`: ").len()..];
            assert!(!reason.is_empty() && !reason.starts_with(char::is_uppercase), "{e}");
            assert!(!reason.contains("report this to the developer"), "{e}");
        }
    }

    #[test]
    fn the_reason_is_said_in_words() {
        assert_eq!(parse("a[").unwrap_err(), "invalid CSS selector `a[`: it ends too early");
        assert_eq!(parse("p >").unwrap_err(), "invalid CSS selector `p >`: dangling combinator");
        assert_eq!(
            parse("p:frobnicate").unwrap_err(),
            "invalid CSS selector `p:frobnicate`: unsupported pseudo class or element"
        );
    }

    #[test]
    fn a_kind_is_said_in_lowercase_words() {
        assert_eq!(words("DanglingCombinator"), "dangling combinator");
        assert_eq!(words("ClassNeedsIdent(Delim('.'))"), "class needs ident");
        assert_eq!(words(""), "");
        assert_eq!(lowercase_first("Token \"]\" was not expected"), "token \"]\" was not expected");
        assert_eq!(lowercase_first(""), "it is malformed");
    }

    #[test]
    fn a_selector_too_long_is_refused_before_it_is_parsed() {
        let longest = format!("p{}", " p".repeat((MAX_LENGTH - 1) / 2));
        assert!(longest.len() <= MAX_LENGTH && parse(&longest).is_ok());
        let longer = format!("{longest} p");
        assert_eq!(
            parse(&longer).unwrap_err(),
            "invalid CSS selector `p p p p p p p p p p p p p p p p p p p p …`: it is too long (more than 4096 bytes)"
        );
        // a hundred thousand compound selectors: an error, not a stack overflow
        assert!(parse(&vec!["div"; 100_000].join(" ")).unwrap_err().ends_with("it is too long (more than 4096 bytes)"));
    }

    #[test]
    fn a_selector_nesting_too_deeply_is_refused_before_it_is_parsed() {
        let nested = |n: usize| format!("{}p{}", ":not(".repeat(n), ")".repeat(n));
        assert!(parse(&nested(MAX_NESTING)).is_ok());
        assert_eq!(
            parse(&nested(MAX_NESTING + 1)).unwrap_err(),
            "invalid CSS selector `:not(:not(:not(:not(:not(:not(:not(:not(…`: \
             it nests too deeply (more than 32 parentheses inside one another)"
        );
        // short enough, but deeper than a test thread's stack could parse
        assert!(parse(&nested(600)).unwrap_err().ends_with("(more than 32 parentheses inside one another)"));
        assert!(parse(&nested(200_000)).unwrap_err().ends_with("it is too long (more than 4096 bytes)"));
    }

    #[test]
    fn parentheses_in_strings_or_escaped_do_not_nest() {
        assert_eq!(nesting("a:not(.b):is(c, :has(> d))"), 2);
        assert_eq!(nesting("[title=\"((((\"]"), 0);
        assert_eq!(nesting("[title='(\\'(']"), 0);
        assert_eq!(nesting(".a\\(b"), 0);
        assert_eq!(nesting("))(("), 2);
        let quoted = format!("[title=\"{}\"]", "(".repeat(100));
        assert!(parse(&quoted).is_ok());
    }

    #[test]
    fn an_excerpt_quotes_the_start_on_a_character_boundary() {
        assert_eq!(excerpt("short"), "short");
        assert_eq!(excerpt(&"é".repeat(50)), format!("{}…", "é".repeat(40)));
    }

    #[test]
    fn an_empty_selector_is_an_error() {
        assert_eq!(parse("").unwrap_err(), "invalid CSS selector ``: it is empty");
        assert_eq!(parse("   ").unwrap_err(), "invalid CSS selector ``: it is empty");
    }
}
