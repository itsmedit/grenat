//! Templates: text with `{{name}}` substitutions and `{% for x in list %}`
//! … `{% end %}` repetitions.

use std::collections::HashMap;

/// The value of a template variable.
#[derive(Debug, Clone, PartialEq)]
pub enum Binding {
    One(String),
    /// A `*variadic` parameter: `{{list}}` joins it with `, `.
    Many(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
enum Segment {
    Text(String),
    Subst(String),
    For { var: String, list: String, body: Vec<Segment> },
}

/// `template` with its variables replaced.
pub fn render(template: &str, bindings: &HashMap<String, Binding>) -> Result<String, String> {
    let (segments, rest) = segments(template, false)?;
    debug_assert!(rest.is_empty());
    let mut out = String::new();
    write(&segments, bindings, &mut out)?;
    Ok(out)
}

fn write(segments: &[Segment], bindings: &HashMap<String, Binding>, out: &mut String) -> Result<(), String> {
    for segment in segments {
        match segment {
            Segment::Text(text) => out.push_str(text),
            Segment::Subst(name) => match bindings.get(name) {
                Some(Binding::One(value)) => out.push_str(value),
                Some(Binding::Many(values)) => out.push_str(&values.join(", ")),
                None => return Err(format!("unknown name `{name}` in `{{{{{name}}}}}`")),
            },
            Segment::For { var, list, body } => {
                let items = match bindings.get(list) {
                    Some(Binding::Many(items)) => items,
                    Some(Binding::One(_)) => return Err(format!("`{list}` is not a list: declare it `*{list}`")),
                    None => return Err(format!("unknown name `{list}` in `{{% for {var} in {list} %}}`")),
                };
                for item in items {
                    let mut inner = bindings.clone();
                    inner.insert(var.clone(), Binding::One(item.clone()));
                    write(body, &inner, out)?;
                }
            }
        }
    }
    Ok(())
}

/// The segments of `text`, up to `{% end %}` when `nested`; and what follows it.
fn segments(mut text: &str, nested: bool) -> Result<(Vec<Segment>, &str), String> {
    let mut out = Vec::new();
    loop {
        let next = [text.find("{{"), text.find("{%")].into_iter().flatten().min();
        let Some(at) = next else {
            if nested {
                return Err("`{% for … %}` without `{% end %}`".into());
            }
            push_text(&mut out, text);
            return Ok((out, ""));
        };
        if text[at..].starts_with("{{") {
            push_text(&mut out, &text[..at]);
            let close = text[at..].find("}}").ok_or("`{{` without `}}`")? + at;
            let name = text[at + 2..close].trim();
            if !is_name(name) {
                return Err(format!("`{{{{{name}}}}}`: expected a name"));
            }
            out.push(Segment::Subst(name.to_string()));
            text = &text[close + 2..];
            continue;
        }
        let close = text[at..].find("%}").ok_or("`{%` without `%}`")? + at;
        let tag = text[at + 2..close].trim();
        // a tag alone on its line takes the line with it
        let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[close..].find('\n').map_or(text.len(), |i| close + i);
        let alone = text[line_start..at].trim().is_empty() && text[close + 2..line_end].trim().is_empty();
        let (before, after) = if alone {
            (&text[..line_start], &text[(line_end + 1).min(text.len())..])
        } else {
            (&text[..at], &text[close + 2..])
        };
        push_text(&mut out, before);
        let words: Vec<&str> = tag.split_whitespace().collect();
        match words.as_slice() {
            ["end"] if nested => return Ok((out, after)),
            ["end"] => return Err("`{% end %}` without `{% for … %}`".into()),
            ["for", var, "in", list] if is_name(var) && is_name(list) => {
                let (body, rest) = segments(after, true)?;
                out.push(Segment::For { var: var.to_string(), list: list.to_string(), body });
                text = rest;
            }
            _ => return Err(format!("unknown tag `{{% {tag} %}}`: expected `{{% for x in list %}}` or `{{% end %}}`")),
        }
    }
}

fn push_text(out: &mut Vec<Segment>, text: &str) {
    if !text.is_empty() {
        out.push(Segment::Text(text.to_string()));
    }
}

fn is_name(s: &str) -> bool {
    s.starts_with(|c: char| c.is_alphabetic() || c == '_') && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings(pairs: &[(&str, Binding)]) -> HashMap<String, Binding> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn many(items: &[&str]) -> Binding {
        Binding::Many(items.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn substitutions_and_loops() {
        let b = bindings(&[("name", Binding::One("title".into())), ("names", many(&["a", "b"]))]);
        assert_eq!(render("def {{name}} = @{{ name }}", &b).unwrap(), "def title = @title");
        assert_eq!(render("[{{names}}]", &b).unwrap(), "[a, b]");
        let template = "start\n  {% for n in names %}\n  def {{n}}_{{name}} = 1\n  {% end %}\nend\n";
        assert_eq!(render(template, &b).unwrap(), "start\n  def a_title = 1\n  def b_title = 1\nend\n");
        assert_eq!(render("x{% for n in names %}{{n}};{% end %}y", &b).unwrap(), "xa;b;y");
        let nested = "{% for a in names %}{% for b in names %}{{a}}{{b}} {% end %}{% end %}";
        assert_eq!(render(nested, &b).unwrap(), "aa ab ba bb ");
    }

    #[test]
    fn template_errors() {
        let b = bindings(&[("one", Binding::One("x".into()))]);
        let err = |t: &str| render(t, &b).unwrap_err();
        assert_eq!(err("{{nope}}"), "unknown name `nope` in `{{nope}}`");
        assert_eq!(err("{{one"), "`{{` without `}}`");
        assert_eq!(err("{{ a b }}"), "`{{a b}}`: expected a name");
        assert_eq!(err("{% for x in one %}{% end %}"), "`one` is not a list: declare it `*one`");
        assert_eq!(err("{% for x in one %}"), "`{% for … %}` without `{% end %}`");
        assert_eq!(err("{% end %}"), "`{% end %}` without `{% for … %}`");
        assert!(err("{% if x %}").starts_with("unknown tag `{% if x %}`"));
    }
}
