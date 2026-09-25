//! `?` placeholders, numbered for PostgreSQL (`$1`, `$2`…), outside of
//! string literals, quoted identifiers and comments.

pub(crate) fn numbered(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut n = 0;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                out.push(c);
                for inner in chars.by_ref() {
                    out.push(inner);
                    if inner == c {
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                out.push(c);
                for inner in chars.by_ref() {
                    out.push(inner);
                    if inner == '\n' {
                        break;
                    }
                }
            }
            '?' => {
                n += 1;
                out.push_str(&format!("${n}"));
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::numbered;

    #[test]
    fn placeholders_are_numbered_outside_literals() {
        assert_eq!(numbered("select * from t where a = ? and b = ?"), "select * from t where a = $1 and b = $2");
        assert_eq!(numbered("select '?', \"a?\" -- ?\n, ?"), "select '?', \"a?\" -- ?\n, $1");
    }
}
