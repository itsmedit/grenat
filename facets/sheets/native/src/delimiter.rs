//! The delimiter of a CSV text: one ASCII character (`,`, `;`, `\t`, `|`),
//! never a quote or a line break, which CSV keeps for itself.

/// The byte `text` names as a delimiter.
pub fn delimiter(text: &str) -> Result<u8, String> {
    match text.as_bytes() {
        [b'"' | b'\n' | b'\r'] => Err(format!("{text:?} cannot be a delimiter: CSV uses it for quoting and lines")),
        [byte] if byte.is_ascii() => Ok(*byte),
        _ => Err(format!("a delimiter is one ASCII character, such as \",\", \";\" or \"\\t\": not {text:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_ascii_character() {
        assert_eq!(delimiter(","), Ok(b','));
        assert_eq!(delimiter(";"), Ok(b';'));
        assert_eq!(delimiter("\t"), Ok(b'\t'));
        assert!(delimiter("").unwrap_err().starts_with("a delimiter is one ASCII character"));
        assert!(delimiter(",,").unwrap_err().ends_with("not \",,\""));
        assert!(delimiter("é").is_err());
        assert!(delimiter("\"").unwrap_err().contains("cannot be a delimiter"));
        assert!(delimiter("\n").is_err());
    }
}
