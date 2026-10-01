//! Parsing a CSS selector: a selector that does not parse is an error
//! message naming it, never a panic.

use scraper::Selector;
use scraper::error::SelectorErrorKind;

/// The selector `text`, parsed, or why it is not one.
pub fn parse(text: &str) -> Result<Selector, String> {
    if text.trim().is_empty() {
        return Err("invalid CSS selector ``: it is empty".to_string());
    }
    Selector::parse(text).map_err(|e| format!("invalid CSS selector `{text}`: {}", reason(&e)))
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
    fn an_empty_selector_is_an_error() {
        assert_eq!(parse("").unwrap_err(), "invalid CSS selector ``: it is empty");
        assert_eq!(parse("   ").unwrap_err(), "invalid CSS selector ``: it is empty");
    }
}
