//! `config/models.yml`: the application's models, and the declarations they
//! stand for.
//!
//! ```yaml
//! fast:                       # the first model is the default one
//!   provider: anthropic
//!   name: claude-haiku-4-5
//! smart:
//!   provider: openai
//!   name: gpt-5
//!   price: {input: 1.25, output: 10}
//! ```
//!
//! Each entry becomes `model :fast, provider: :anthropic, name: '…'`, so the
//! checker and the runtime see models declared as in code, and report a
//! mistake as they would there.

use yaml_rust2::Yaml;

/// Where the models are, in an application.
pub const FILE: &str = "config/models.yml";

/// Options whose value is a name (a symbol), not a text.
const SYMBOLS: [&str; 2] = ["provider", "effort"];

/// The declarations `yaml` stands for, one per line, in its order.
pub fn declarations(yaml: &str) -> Result<String, String> {
    let Yaml::Hash(models) = crate::yaml::document(yaml)? else {
        return Err("the models are a mapping: `fast: {provider: anthropic, name: …}`".into());
    };
    let mut out = String::new();
    for (name, options) in &models {
        let name = key(name)
            .filter(|n| valid_name(n))
            .ok_or_else(|| format!("`{}` is not a model name: lowercase letters, digits and `_`", shown(name)))?;
        let Yaml::Hash(options) = options else {
            return Err(format!("model `{name}`: its options are a mapping (provider, name…)"));
        };
        let mut line = format!("model :{name}");
        for (option, value) in options {
            let option = key(option)
                .filter(|o| valid_name(o))
                .ok_or_else(|| format!("model `{name}`: `{}` is not an option name", shown(option)))?;
            if matches!(value, Yaml::Null) {
                continue;
            }
            let value = match value {
                Yaml::String(text) if SYMBOLS.contains(&option.as_str()) => {
                    if !valid_name(text) {
                        return Err(format!("model `{name}`: `{option}: {text}` is not a name"));
                    }
                    format!(":{text}")
                }
                other => literal(other).map_err(|e| format!("model `{name}`, `{option}`: {e}"))?,
            };
            line.push_str(&format!(", {option}: {value}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

fn key(yaml: &Yaml) -> Option<String> {
    match yaml {
        Yaml::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn shown(yaml: &Yaml) -> String {
    match yaml {
        Yaml::String(s) | Yaml::Real(s) => s.clone(),
        Yaml::Integer(n) => n.to_string(),
        Yaml::Boolean(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

/// A value as Grenat source: texts are raw strings (no interpolation).
fn literal(value: &Yaml) -> Result<String, String> {
    Ok(match value {
        Yaml::Null => "nil".into(),
        Yaml::Boolean(b) => b.to_string(),
        Yaml::Integer(n) => n.to_string(),
        Yaml::Real(text) => {
            text.parse::<f64>().map(|_| text.clone()).map_err(|_| format!("`{text}` is not a number"))?
        }
        Yaml::String(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        Yaml::Array(items) => format!("[{}]", items.iter().map(literal).collect::<Result<Vec<_>, _>>()?.join(", ")),
        Yaml::Hash(pairs) => {
            let mut parts = Vec::new();
            for (k, v) in pairs {
                let k = key(k).filter(|k| valid_name(k)).ok_or_else(|| format!("`{}` is not a key name", shown(k)))?;
                parts.push(format!("{k}: {}", literal(v)?));
            }
            format!("{{{}}}", parts.join(", "))
        }
        Yaml::Alias(_) | Yaml::BadValue => return Err("unsupported YAML value".into()),
    })
}

fn valid_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_become_declarations() {
        let yaml = "\
# the first is the default
fast:
  provider: anthropic
  name: claude-haiku-4-5
  temperature: 0.2
smart:
  provider: openai
  name: gpt-5
  effort: low
  max_tokens: 4000
  price: {input: 1.25, output: 10}
local:
  provider: ollama
  name: \"it's #{here}\"
  base_url: http://gpu:11434/v1
  fallbacks: false
  temperature:
";
        assert_eq!(
            declarations(yaml).unwrap(),
            "\
model :fast, provider: :anthropic, name: 'claude-haiku-4-5', temperature: 0.2
model :smart, provider: :openai, name: 'gpt-5', effort: :low, max_tokens: 4000, price: {input: 1.25, output: 10}
model :local, provider: :ollama, name: 'it\\'s #{here}', base_url: 'http://gpu:11434/v1', fallbacks: false
"
        );
        assert_eq!(declarations("").unwrap(), "");
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(declarations("- a\n").unwrap_err().contains("a mapping"));
        assert!(declarations("Fast:\n  provider: anthropic\n").unwrap_err().contains("not a model name"));
        assert!(declarations("fast: claude\n").unwrap_err().contains("its options are a mapping"));
        assert!(declarations("fast:\n  provider: open ai\n").unwrap_err().contains("is not a name"));
        assert!(declarations("fast:\n  Name: x\n").unwrap_err().contains("not an option name"));
    }
}
