//! The names a generator derives from the one it is given.

/// `support_desk` → `SupportDesk` (a type), `support_desks` (a table).
#[derive(Debug, Clone, PartialEq)]
pub struct Names {
    pub snake: String,
    pub camel: String,
    pub plural: String,
}

impl Names {
    pub fn new(name: &str) -> Result<Names, String> {
        if !valid(name) {
            return Err(format!(
                "invalid name `{name}`: use lowercase letters, digits and `_`, starting with a letter"
            ));
        }
        Ok(Names { snake: name.to_string(), camel: camel(name), plural: plural(name) })
    }
}

/// `snake_case`, starting with a letter.
pub fn valid(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.ends_with('_')
        && !name.contains("__")
}

fn camel(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map(|first| first.to_ascii_uppercase().to_string() + chars.as_str()).unwrap_or_default()
        })
        .collect()
}

/// English plurals, as far as table names need them.
fn plural(name: &str) -> String {
    let vowel_before_y = name.len() > 1 && name[..name.len() - 1].ends_with(['a', 'e', 'i', 'o', 'u']);
    if name.ends_with('y') && !vowel_before_y {
        format!("{}ies", &name[..name.len() - 1])
    } else if ["s", "x", "z", "ch", "sh"].iter().any(|end| name.ends_with(end)) {
        format!("{name}es")
    } else {
        format!("{name}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let names = Names::new("support_desk").unwrap();
        assert_eq!((names.camel.as_str(), names.plural.as_str()), ("SupportDesk", "support_desks"));
        assert_eq!(Names::new("category").unwrap().plural, "categories");
        assert_eq!(Names::new("day").unwrap().plural, "days");
        assert_eq!(Names::new("status").unwrap().plural, "statuses");
        assert_eq!(Names::new("box").unwrap().plural, "boxes");
        assert_eq!(Names::new("v2_agent").unwrap().camel, "V2Agent");
        for bad in ["Triage", "2fa", "a-b", "", "a__b", "a_"] {
            assert!(Names::new(bad).is_err(), "{bad}");
        }
    }
}
