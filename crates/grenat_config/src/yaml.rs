//! YAML, read into JSON trees (one document per file).

use serde_json::{Map, Value as Json};
use yaml_rust2::{Yaml, YamlLoader};

/// The document of `text`: an empty one is an empty mapping.
pub fn parse(text: &str) -> Result<Json, String> {
    json(&document(text)?)
}

/// The document of `text`, as YAML (mappings keep their order).
pub(crate) fn document(text: &str) -> Result<Yaml, String> {
    let mut docs = YamlLoader::load_from_str(text).map_err(|e| format!("invalid YAML: {e}"))?;
    match docs.len() {
        0 => Ok(Yaml::Hash(Default::default())),
        1 => Ok(docs.remove(0)),
        _ => Err("a configuration file holds one YAML document".into()),
    }
}

fn json(yaml: &Yaml) -> Result<Json, String> {
    Ok(match yaml {
        Yaml::Null => Json::Null,
        Yaml::Boolean(b) => Json::Bool(*b),
        Yaml::Integer(n) => Json::from(*n),
        Yaml::Real(text) => text.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map_or(Json::String(text.clone()), Json::Number),
        Yaml::String(s) => Json::String(s.clone()),
        Yaml::Array(items) => Json::Array(items.iter().map(json).collect::<Result<_, _>>()?),
        Yaml::Hash(pairs) => {
            let mut map = Map::new();
            for (key, value) in pairs {
                let key = match key {
                    Yaml::String(s) => s.clone(),
                    Yaml::Integer(n) => n.to_string(),
                    Yaml::Boolean(b) => b.to_string(),
                    other => return Err(format!("unsupported key {other:?}")),
                };
                map.insert(key, json(value)?);
            }
            Json::Object(map)
        }
        Yaml::Alias(_) | Yaml::BadValue => return Err("unsupported YAML value (an alias?)".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn documents() {
        let doc = parse("# comment\nfast:\n  provider: anthropic\n  temperature: 0.2\n  max_tokens: 800\n  tags: [a, b]\n  batch: true\nempty:\n").unwrap();
        assert_eq!(doc, json!({"fast": {"provider": "anthropic", "temperature": 0.2, "max_tokens": 800, "tags": ["a", "b"], "batch": true}, "empty": null}));
        assert_eq!(parse("").unwrap(), json!({}));
        assert_eq!(parse("# only comments\n").unwrap(), json!({}));
        assert!(parse("a: [").unwrap_err().starts_with("invalid YAML"));
        assert!(parse("a: 1\n---\nb: 2\n").is_err());
    }
}
