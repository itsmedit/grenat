//! The text of an agent's answer while it is being written: the model
//! gives it as the input of its `final_answer` tool, `{"value": "…"}`, in
//! pieces of JSON; this reads the string of `value` out of them as they
//! come — escapes decoded, even split between two pieces — and nothing
//! else of the object.

/// Where the reading is, in the object.
#[derive(Debug, Clone, PartialEq)]
enum State {
    /// Before the object's `{`.
    Start,
    /// Before a key (or the end).
    Key,
    InKey {
        key: String,
        escaped: bool,
    },
    /// After a key, before its `:`.
    Colon {
        value: bool,
    },
    /// Before a value.
    Value {
        value: bool,
    },
    /// In the string of `value`: given out.
    Text,
    /// In a `\` escape of it: what follows the backslash so far.
    Escape(String),
    /// After a `\uD800`-style high surrogate, waiting for its low half.
    Surrogate {
        high: u32,
        seen: String,
    },
    /// In another value, skipped: its nesting, and whether in a string.
    Skip {
        depth: usize,
        string: bool,
        escaped: bool,
    },
    /// The text is whole (or there is none to find).
    Done,
}

pub(crate) struct AnswerText {
    state: State,
}

impl AnswerText {
    pub(crate) fn new() -> Self {
        AnswerText { state: State::Start }
    }

    /// The text that `piece` adds to the answer.
    pub(crate) fn feed(&mut self, piece: &str) -> String {
        let mut out = String::new();
        for c in piece.chars() {
            self.step(c, &mut out);
        }
        out
    }

    fn step(&mut self, c: char, out: &mut String) {
        let state = std::mem::replace(&mut self.state, State::Done);
        self.state = match state {
            State::Start if c == '{' => State::Key,
            State::Start if c.is_whitespace() => State::Start,
            State::Key if c == '"' => State::InKey { key: String::new(), escaped: false },
            State::Key if c == ',' || c.is_whitespace() => State::Key,
            State::InKey { key, escaped: true } => State::InKey { key: key + &c.to_string(), escaped: false },
            State::InKey { key, .. } if c == '\\' => State::InKey { key, escaped: true },
            State::InKey { key, .. } if c == '"' => State::Colon { value: key == "value" },
            State::InKey { key, .. } => State::InKey { key: key + &c.to_string(), escaped: false },
            State::Colon { value } if c == ':' => State::Value { value },
            State::Colon { value } if c.is_whitespace() => State::Colon { value },
            State::Value { value } if c.is_whitespace() => State::Value { value },
            State::Value { value: true } if c == '"' => State::Text,
            // `value` is not a string: no text to give
            State::Value { value: true } => State::Done,
            State::Value { value: false } => match c {
                '"' => State::Skip { depth: 0, string: true, escaped: false },
                '{' | '[' => State::Skip { depth: 1, string: false, escaped: false },
                _ => State::Skip { depth: 0, string: false, escaped: false },
            },
            State::Skip { depth, string: true, escaped } => match c {
                _ if escaped => State::Skip { depth, string: true, escaped: false },
                '\\' => State::Skip { depth, string: true, escaped: true },
                '"' if depth == 0 => State::Key,
                '"' => State::Skip { depth, string: false, escaped: false },
                _ => State::Skip { depth, string: true, escaped: false },
            },
            State::Skip { depth, string: false, .. } => match c {
                '"' => State::Skip { depth, string: true, escaped: false },
                '{' | '[' => State::Skip { depth: depth + 1, string: false, escaped: false },
                '}' | ']' if depth == 1 => State::Key,
                '}' | ']' if depth > 1 => State::Skip { depth: depth - 1, string: false, escaped: false },
                // the end of a number, `true`… (or of the object)
                ',' if depth == 0 => State::Key,
                '}' => State::Done,
                _ => State::Skip { depth, string: false, escaped: false },
            },
            State::Text => match c {
                '"' => State::Done,
                '\\' => State::Escape(String::new()),
                _ => {
                    out.push(c);
                    State::Text
                }
            },
            State::Escape(seen) => self.escape(seen, c, out),
            State::Surrogate { high, seen } => {
                let seen = seen + &c.to_string();
                if !"\\u".starts_with(&seen) && !(seen.starts_with("\\u") && seen.len() <= 6) {
                    // not followed by its low half: a replacement character, then this one again
                    out.push(char::REPLACEMENT_CHARACTER);
                    self.state = State::Text;
                    for c in seen.chars() {
                        self.step(c, out);
                    }
                    return;
                }
                if seen.len() < 6 {
                    State::Surrogate { high, seen }
                } else {
                    let low = u32::from_str_radix(&seen[2..], 16).unwrap_or(0);
                    if (0xDC00..0xE000).contains(&low) {
                        let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                        out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
                        State::Text
                    } else {
                        out.push(char::REPLACEMENT_CHARACTER);
                        self.state = State::Text;
                        for c in seen.chars() {
                            self.step(c, out);
                        }
                        return;
                    }
                }
            }
            // anything else: not the object expected
            _ => State::Done,
        };
    }

    /// A character of a `\` escape.
    fn escape(&mut self, seen: String, c: char, out: &mut String) -> State {
        if seen.is_empty() {
            let simple = match c {
                'n' => Some('\n'),
                't' => Some('\t'),
                'r' => Some('\r'),
                'b' => Some('\u{8}'),
                'f' => Some('\u{c}'),
                'u' => None,
                other => Some(other),
            };
            return match simple {
                Some(decoded) => {
                    out.push(decoded);
                    State::Text
                }
                None => State::Escape("u".into()),
            };
        }
        let seen = seen + &c.to_string();
        if seen.len() < 5 {
            return State::Escape(seen);
        }
        let code = u32::from_str_radix(&seen[1..], 16).unwrap_or(0xFFFD);
        if (0xD800..0xDC00).contains(&code) {
            return State::Surrogate { high: code, seen: String::new() };
        }
        out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
        State::Text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text read when `json` comes in pieces of `size` characters.
    fn read(json: &str, size: usize) -> String {
        let chars: Vec<char> = json.chars().collect();
        let mut reader = AnswerText::new();
        chars.chunks(size).map(|piece| reader.feed(&piece.iter().collect::<String>())).collect()
    }

    #[test]
    fn the_text_of_value_whatever_the_pieces() {
        let answer = "Ligne 1\n\t\"cité\" \\ é 😀 日本";
        let json = serde_json::json!({"value": answer}).to_string();
        let escaped = r#"{"value": "Ligne 1\n\t\"cité\" \\ é 😀 日本"}"#;
        for size in 1..8 {
            assert_eq!(read(&json, size), answer, "{size}");
            assert_eq!(read(escaped, size), answer, "{size}");
        }
    }

    #[test]
    fn other_keys_are_skipped_and_other_values_give_nothing() {
        let json = r#" {"note": "a \"value\": x", "n": [1, {"value": "no"}], "ok": true, "value": "yes"}"#;
        assert_eq!(read(json, 3), "yes");
        assert_eq!(read(r#"{"value": 42}"#, 1), "");
        assert_eq!(read(r#"{"value": {"value": "inner"}}"#, 1), "");
        assert_eq!(read("not json", 2), "");
        // a lone high surrogate
        assert_eq!(read(r#"{"value": "a\ud83dz"}"#, 1), "a\u{fffd}z");
    }

    #[test]
    fn a_piece_gives_what_it_completes() {
        let mut reader = AnswerText::new();
        assert_eq!(reader.feed("{\"val"), "");
        assert_eq!(reader.feed("ue\": \"Hel"), "Hel");
        assert_eq!(reader.feed("lo \\u00"), "lo ");
        assert_eq!(reader.feed("e9!\"}"), "é!");
        assert_eq!(reader.feed(" more"), "");
    }
}
