//! The formatted source must be the same program, with the same comments.

/// `Ok` if `after` parses to the tree of `before` (positions aside) and
/// keeps all its comments.
pub(crate) fn same_program(before: &str, after: &str) -> Result<(), String> {
    let (a, b) = (grenat_parser::parse(before), grenat_parser::parse(after));
    if !b.diagnostics.is_empty() {
        return Err(format!("the result does not parse: {}", b.diagnostics[0].message));
    }
    if without_spans(&format!("{:?}", a.program)) != without_spans(&format!("{:?}", b.program)) {
        return Err("the result is a different program".into());
    }
    let comments = |src: &str| {
        let mut texts: Vec<String> = grenat_lexer::lex(src).comments.iter().map(|c| c.text.clone()).collect();
        texts.sort();
        texts
    };
    if comments(before) != comments(after) {
        return Err("comments were lost".into());
    }
    Ok(())
}

/// The debug text of a tree with its positions (`Span { start: …, end: … }`) removed.
fn without_spans(debug: &str) -> String {
    let mut out = String::with_capacity(debug.len());
    let mut rest = debug;
    while let Some(i) = rest.find("Span { start: ") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let end = after.find('}').map_or(after.len(), |j| j + 1);
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}
