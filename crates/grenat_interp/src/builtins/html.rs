//! `Html.text(html)`: the text a page shows — no tags, scripts, styles or
//! comments; entities decoded; a line per block; blank space collapsed.
//! What an untrusted page says stays untrusted.

/// Tags after which the text goes to a new line.
const BLOCKS: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "tr",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "section",
    "article",
    "header",
    "footer",
    "pre",
    "blockquote",
    "hr",
    "title",
];

pub(crate) fn text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while i < html.len() {
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            i += rest.find("-->").map_or(rest.len(), |e| e + 3);
        } else if rest.starts_with('<') {
            let end = rest.find('>').map_or(rest.len(), |e| e + 1);
            let tag = rest[1..end.saturating_sub(1)].trim_start_matches('/').to_ascii_lowercase();
            let name: String = tag.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if name == "script" || name == "style" {
                // skip to the closing tag
                let close = format!("</{name}");
                i += lower[i..]
                    .find(&close)
                    .map_or(rest.len(), |c| c + lower[i + c..].find('>').map_or(close.len(), |g| g + 1));
                continue;
            }
            if BLOCKS.contains(&name.as_str()) {
                out.push('\n');
            } else {
                out.push(' ');
            }
            i += end;
        } else if rest.starts_with('&') {
            let (decoded, used) = entity(rest);
            out.push_str(&decoded);
            i += used;
        } else {
            let c = rest.chars().next().expect("not at the end");
            out.push(c);
            i += c.len_utf8();
        }
    }
    collapse(&out)
}

/// An entity at the start of `text`: what it stands for, and its length.
fn entity(text: &str) -> (String, usize) {
    let Some(end) = text.find(';').filter(|e| *e <= 10) else { return ("&".into(), 1) };
    let name = &text[1..end];
    let decoded = match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => name
            .strip_prefix("#x")
            .or_else(|| name.strip_prefix("#X"))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
            .and_then(char::from_u32),
    };
    match decoded {
        Some(c) => (c.to_string(), end + 1),
        None => ("&".into(), 1),
    }
}

/// Spaces within a line collapsed, blank lines dropped.
fn collapse(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::text;

    #[test]
    fn a_page_as_text() {
        let html = "<html><head><title>Rust</title><style>p { color: red }</style>\
            <script>alert('<p>no</p>')</script></head><body><!-- nav -->\
            <h1>Why&nbsp;Rust</h1><p>Memory <b>safe</b> &amp; fast &#8212; &#x2713;</p>\
            <ul><li>one</li><li>two</li></ul>Tom &amp Jerry &lt;3</body></html>";
        assert_eq!(text(html), "Rust\nWhy Rust\nMemory safe & fast — ✓\none\ntwo\nTom &amp Jerry <3");
        assert_eq!(text("plain text"), "plain text");
        assert_eq!(text("<SCRIPT>x</SCRIPT>ok"), "ok");
    }
}
