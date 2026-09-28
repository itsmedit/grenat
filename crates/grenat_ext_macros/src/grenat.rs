//! What Grenat's grammar reserves, as the macros check it at compile time:
//! a function's name is never a keyword (a parameter or a field may be: a
//! label, or a name after a `.`), an effect is one Grenat knows. The lists are
//! Grenat's own (`grenat_lexer`, `grenat_ast`: the tests compare them), so
//! that a facet does not build a library whose declarations break every
//! program that loads them.

/// Grenat's keywords.
pub(crate) const KEYWORDS: &[&str] = &[
    "abstract",
    "agent",
    "and",
    "begin",
    "break",
    "case",
    "class",
    "def",
    "do",
    "else",
    "elsif",
    "end",
    "ensure",
    "enum",
    "false",
    "if",
    "in",
    "module",
    "next",
    "nil",
    "not",
    "or",
    "prompt",
    "rescue",
    "return",
    "self",
    "struct",
    "supervisor",
    "then",
    "tool",
    "true",
    "unless",
    "until",
    "when",
    "while",
    "workflow",
];

/// The effects Grenat knows, by their path (`net("api.x.com")` is `net`).
pub(crate) const EFFECTS: &[&str] = &[
    "llm", "net", "fs", "fs.read", "fs.write", "db", "db.read", "db.write", "mcp", "shell", "ssh", "human", "time",
    "random", "env",
];

/// Why `name` cannot be a Grenat function's name, if it cannot.
pub(crate) fn check_function_name(name: &str) -> Result<(), String> {
    let shape = name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !shape {
        Err(format!("`{name}` is not a Grenat name: lowercase letters, digits and `_`"))
    } else if KEYWORDS.contains(&name) {
        Err(format!("`{name}` is a Grenat keyword: Grenat code could not name it"))
    } else {
        Ok(())
    }
}

/// Whether the path of `effect` (`net` for `net("api.x.com")`) is one Grenat knows.
pub(crate) fn is_known_effect(effect: &str) -> bool {
    EFFECTS.contains(&effect.split('(').next().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_are_grenats() {
        let mut keywords: Vec<&str> = grenat_lexer::Keyword::ALL.iter().map(|k| k.as_str()).collect();
        keywords.sort_unstable();
        let mut ours = KEYWORDS.to_vec();
        ours.sort_unstable();
        assert_eq!(ours, keywords);
        assert_eq!(EFFECTS, grenat_ast::KNOWN_EFFECTS);
    }

    #[test]
    fn names_and_effects_are_checked() {
        assert!(check_function_name("read_sheet").is_ok() && check_function_name("_x1").is_ok());
        assert_eq!(check_function_name("end").unwrap_err(), "`end` is a Grenat keyword: Grenat code could not name it");
        assert!(check_function_name("Add").unwrap_err().contains("not a Grenat name"));
        assert!(is_known_effect("fs.read") && is_known_effect("net(\"api.x.com\")"));
        assert!(!is_known_effect("bogus") && !is_known_effect("fs.delete"));
    }
}
