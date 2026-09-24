//! Helpers shared by the checker tests.
#![allow(dead_code)]

use grenat_types::{Diagnostic, check};

pub fn diags(src: &str) -> Vec<Diagnostic> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    check(&parsed.program)
}

pub fn clean(src: &str) {
    let d = diags(src);
    assert!(d.is_empty(), "unexpected diagnostics:\n{}", render(src, &d));
}

/// Exactly one diagnostic, with this code, on this source text.
pub fn single(src: &str, code: &str, at: &str) -> Diagnostic {
    let d = diags(src);
    assert_eq!(d.len(), 1, "expected exactly one diagnostic:\n{}", render(src, &d));
    let diag = d.into_iter().next().unwrap();
    assert_eq!(diag.code, Some(code), "{}", render(src, std::slice::from_ref(&diag)));
    assert_eq!(&src[diag.span.range()], at, "{}", diag.message);
    diag
}

pub fn render(src: &str, diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| format!("  [{}] {} `{}`", d.code.unwrap_or("-"), d.message, &src[d.span.range()]))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn example(name: &str) -> String {
    std::fs::read_to_string(format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

pub const PRELUDE: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
struct Summary
  title: String
  bullets: Array(String)
  def headline = title.upcase
end
prompt summarize(text: String) -> ~Summary using :fast
  user text
end
tool send(to: String, body: String) -> Unit uses net
  puts body
end
";
