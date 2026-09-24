//! All Grenat code in the repository must parse without errors:
//! the programs in `examples/` and every ```ruby block of `SPEC.md`.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn assert_parses(name: &str, src: &str) {
    let parsed = grenat_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "{name} does not parse:\n{}",
        parsed
            .diagnostics
            .iter()
            .map(|d| format!("  - {} (bytes {:?}): \"{}\"", d.message, d.span.range(), &src[d.span.range()]))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(!parsed.program.items.is_empty(), "{name} is empty");
}

#[test]
fn examples_parse() {
    let mut count = 0;
    for entry in fs::read_dir(repo_root().join("examples")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "grn") {
            assert_parses(&path.display().to_string(), &fs::read_to_string(&path).unwrap());
            count += 1;
        }
    }
    assert!(count > 0, "no example found");
}

#[test]
fn spec_code_blocks_parse() {
    let spec = fs::read_to_string(repo_root().join("SPEC.md")).unwrap();
    let mut blocks = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (n, line) in spec.lines().enumerate() {
        match (&mut current, line.trim_start()) {
            (None, "```ruby") => current = Some((n + 1, String::new())),
            (Some(_), "```") => blocks.push(current.take().unwrap()),
            (Some((_, code)), _) => {
                code.push_str(line);
                code.push('\n');
            }
            _ => {}
        }
    }
    assert!(blocks.len() >= 10, "only {} blocks found", blocks.len());
    for (line, code) in blocks {
        assert_parses(&format!("SPEC.md, block at line {line}"), &code);
    }
}

/// Code being typed must never make the parser panic or loop.
#[test]
fn every_prefix_of_the_examples_parses_without_panicking() {
    let src = fs::read_to_string(repo_root().join("examples/support_desk.grn")).unwrap();
    for (i, _) in src.char_indices() {
        let _ = grenat_parser::parse(&src[..i]);
    }
}
