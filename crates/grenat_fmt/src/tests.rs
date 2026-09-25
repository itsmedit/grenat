//! Formatting: the canonical layout, and safety on every real program.

use crate::{FmtError, format};


fn fmt(src: &str) -> String {
    format(src).unwrap_or_else(|e| panic!("{e:?}\n--- source:\n{src}"))
}

fn repo(path: &str) -> String {
    std::fs::read_to_string(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Every code block of the specification.
fn spec_blocks() -> Vec<String> {
    let spec = repo("SPEC.md");
    let mut blocks = Vec::new();
    let mut rest = spec.as_str();
    while let Some(start) = rest.find("```ruby\n") {
        let body = &rest[start + 8..];
        let end = body.find("```").expect("closed block");
        blocks.push(body[..end].to_string());
        rest = &body[end + 3..];
    }
    blocks
}

#[test]
fn every_example_and_spec_block_formats_safely() {
    let mut failures = Vec::new();
    let examples = std::fs::read_dir(format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut sources: Vec<(String, String)> = examples
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "grn"))
        .map(|p| (p.display().to_string(), std::fs::read_to_string(&p).unwrap()))
        .collect();
    sources.extend(spec_blocks().into_iter().enumerate().map(|(i, b)| (format!("SPEC block {i}"), b)));
    for (name, src) in &sources {
        if let Err(e) = format(src) {
            failures.push(format!("{name}: {e:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn messy_code_gets_the_canonical_layout() {
    let messy = "\
def   area( w: Int,h:   Int )->Int
      x=w*h   # the area
    return  x+0 if x>0



   (w+h)*2
end
";
    let canonical = "\
def area(w: Int, h: Int) -> Int
  x = w * h  # the area
  return x + 0 if x > 0

  (w + h) * 2
end
";
    assert_eq!(fmt(messy), canonical);
    assert_eq!(fmt(canonical), canonical);
}

#[test]
fn parentheses_are_the_ones_the_grammar_needs() {
    assert_eq!(fmt("x = ((a + b)) * (c * d)\n"), "x = (a + b) * (c * d)\n");
    assert_eq!(fmt("x = (a * b) * c\n"), "x = a * b * c\n");
    assert_eq!(fmt("x = a - (b - c)\n"), "x = a - (b - c)\n");
    assert_eq!(fmt("x = (a ** b) ** c\n"), "x = (a ** b) ** c\n");
    assert_eq!(fmt("x = -(3).abs\n"), "x = -(3.abs)\n");
    assert_eq!(fmt("x = !(a && b)\n"), "x = !(a && b)\n");
    // `and` binds looser than `=`: the tree keeps it, the output says it
    assert_eq!(fmt("x = a and b\n"), "(x = a) && b\n");
}

#[test]
fn what_the_author_chose_is_kept() {
    let src = "\
xs = [1_000, 2_000]
ys = xs.map { |x| x * 2 }
xs.each do |x|
  puts x unless x > 1
end
s = if ok then \"yes\" else \"no\" end
t = \"tab\\there #{s}\"
";
    assert_eq!(fmt(src), src);
}

#[test]
fn comments_stay_where_they_were() {
    let src = "\
# leading
## Doc of `f`.
def f(n: Int) -> Int  # after the header
  # before the statement
  n + 1  # after it
  # at the end of the body
end
# at the end of the file
";
    assert_eq!(fmt(src), src);
}

#[test]
fn long_lists_and_chains_are_broken() {
    let src = "items = [\"alpha alpha alpha\", \"beta beta beta beta\", \"gamma gamma gamma\", \"delta delta delta delta delta\"]\n";
    assert_eq!(
        fmt(src),
        "items = [\n  \"alpha alpha alpha\",\n  \"beta beta beta beta\",\n  \"gamma gamma gamma\",\n  \"delta delta delta delta delta\",\n]\n"
    );
    let chain = "result = records.select { |r| r.valid_and_complete? }.map { |r| r.transform_into_output }.sort_by { |r| r.key }\n";
    let formatted = fmt(chain);
    assert!(formatted.starts_with("result = records\n  .select"), "{formatted}");
    assert!(formatted.lines().all(|l| l.len() <= 100), "{formatted}");
}

#[test]
fn heredocs_are_reindented_with_their_statement() {
    let src = "\
def f
      text = <<~T
            Hello #{name}
              indented
          T
  text
end
";
    assert_eq!(fmt(src), "def f\n  text = <<~T\n    Hello #{name}\n      indented\n  T\n  text\nend\n");
}

#[test]
fn a_file_that_does_not_parse_is_left_alone() {
    assert_eq!(format("def f(\n"), Err(FmtError::Syntax));
}

#[test]
fn a_hash_argument_is_parenthesized_only_without_call_parentheses() {
    assert_eq!(fmt("x = h.merge({b: 2})\n"), "x = h.merge({b: 2})\n");
    // without them, `{` would open a block
    assert_eq!(fmt("show ({a: 1})\n"), "show ({a: 1})\n");
    assert_eq!(fmt("show \"merge\", {a: 1}.merge({b: 2})\n"), "show \"merge\", {a: 1}.merge({b: 2})\n");
}

#[test]
fn hash_shorthand_is_kept() {
    let src = "query = \"x\"\np({query:, n: 1})\nx = {query:}\n";
    assert_eq!(crate::format(src).unwrap(), src);
}

#[test]
fn ternaries_are_kept() {
    let src = "p x > 2 ? \"big\" : \"small\"\ny = a ? b : c ? d : e\nz = (c ? 1 : 2) + 3\n";
    assert_eq!(crate::format(src).unwrap(), src);
    assert_eq!(crate::format("y = a ?\n  b :\n  c\n").unwrap(), "y = a ? b : c\n");
    assert_eq!(crate::format("if a then b else c end\n").unwrap(), "if a then b else c end\n");
}

#[test]
fn a_parenthesized_if_stays_an_if() {
    let src = "out = out + (if i == 0 then \"\" else \",\" end) + x\n";
    assert_eq!(crate::format(src).unwrap(), src);
}

#[test]
fn constants_are_kept() {
    let src = "module GitHub\n  API = \"https://api.github.com\"\n\n  def self.url = API\nend\n";
    assert_eq!(crate::format(src).unwrap(), src);
}

#[test]
fn top_level_constants_are_kept() {
    let src = "MEMORY = \"memory.json\"\n\ndef path = MEMORY\n";
    assert_eq!(crate::format(src).unwrap(), src);
}
