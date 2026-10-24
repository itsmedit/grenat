//! The references written for LLMs — `llms.txt` (the core) and the topic
//! files in `docs/llms/` — hold code that compiles and tests that pass.
//!
//! Each document's ` ```ruby ` blocks become one program: the blocks that
//! hold a `test "…" do` go into `<doc>_test.grn`, which requires the file
//! made of the others, `<doc>.grn`. `grenat check` runs on the program and
//! `grenat test` on its tests, in a directory of their own; any error fails
//! with the document and the block it points into.
//!
//! The documents show code as a reader writes it in an application, where
//! the models live in `config/models.yml`. So every program starts with
//! [`PREAMBLE`], which declares the models the documents name — `:fast` and
//! `:smart` (chat), `:whisper` (transcription), `:docs` (embeddings) — and
//! the directory holds the folders their examples read and write (`docs/`,
//! `inbox/`, `filed/`).
//!
//! Blocks in another language (`sh`, `yaml`…) are not checked. A ruby block
//! that cannot stand in a program is marked ` ```ruby skip-check ` and the
//! text says why; none is today.
//!
//! The core's index and the topic files must also agree: every topic named
//! in `llms.txt` exists, and every topic file is named there.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// What every document's program starts with (see the module's doc).
const PREAMBLE: &str = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
model :smart, provider: :anthropic, name: \"claude-opus-5\"
model :whisper, provider: :openai, name: \"gpt-transcribe\", kind: :transcription
model :docs, provider: :voyage, name: \"voyage-3.5\", kind: :embedding
";

/// The folders a document's examples use, made next to its program.
const FOLDERS: [&str; 3] = ["docs", "inbox", "filed"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `llms.txt`, then the topic files in name order.
fn documents() -> Vec<PathBuf> {
    let mut topics: Vec<PathBuf> = std::fs::read_dir(root().join("docs/llms"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    topics.sort();
    let mut docs = vec![root().join("llms.txt")];
    docs.extend(topics);
    docs
}

/// A fenced code block: its info string, its first line in the document
/// (1-based, the line after the fence) and its text.
struct Block {
    info: String,
    line: usize,
    text: String,
}

impl Block {
    fn language(&self) -> &str {
        self.info.split_whitespace().next().unwrap_or("")
    }

    fn checked(&self) -> bool {
        self.language() == "ruby" && !self.info.split_whitespace().any(|w| w == "skip-check")
    }

    fn is_test(&self) -> bool {
        self.text.lines().any(|l| l.starts_with("test \""))
    }
}

fn blocks(doc: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut open: Option<Block> = None;
    for (i, line) in doc.lines().enumerate() {
        match open.take() {
            None => {
                if let Some(info) = line.strip_prefix("```") {
                    open = Some(Block { info: info.trim().to_string(), line: i + 2, text: String::new() });
                }
            }
            Some(mut block) => {
                if line.starts_with("```") {
                    out.push(block);
                } else {
                    block.text.push_str(line);
                    block.text.push('\n');
                    open = Some(block);
                }
            }
        }
    }
    assert!(open.is_none(), "a code block is never closed");
    out
}

/// A generated file: its text, and which lines of it come from which block.
struct Source {
    text: String,
    /// (first line in the file, first line in the document) for each block.
    spans: Vec<(usize, usize)>,
}

impl Source {
    fn new(head: &str) -> Source {
        Source { text: head.to_string(), spans: Vec::new() }
    }

    fn add(&mut self, block: &Block) {
        self.text.push('\n');
        let first = self.text.lines().count() + 1;
        self.spans.push((first, block.line));
        self.text.push_str(&block.text);
    }

    /// The document line of the block holding `line` of this file.
    fn block_at(&self, line: usize) -> Option<usize> {
        self.spans.iter().rev().find(|(first, _)| *first <= line).map(|(_, doc_line)| *doc_line)
    }
}

/// The lines of `file` an error output points at (`file:LINE:COL`).
fn lines_named(output: &str, file: &str) -> Vec<usize> {
    let needle = format!("{file}:");
    let mut lines = Vec::new();
    let mut rest = output;
    while let Some(at) = rest.find(&needle) {
        rest = &rest[at + needle.len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse() {
            lines.push(n);
        }
    }
    lines
}

fn grenat(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("VOYAGE_API_KEY")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env_remove("GRENAT_LOG")
        .output()
        .unwrap()
}

/// Checks and tests one document; the failures, described.
fn verify(doc: &Path, work: &Path) -> Vec<String> {
    let name = doc.file_stem().unwrap().to_string_lossy().replace(['-', '.'], "_");
    let shown = doc.strip_prefix(root()).unwrap_or(doc).display().to_string();
    let dir = work.join(&name);
    let _ = std::fs::remove_dir_all(&dir);
    for folder in FOLDERS {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
    }

    let mut program = Source::new(PREAMBLE);
    let mut tests = Source::new(&format!("require \"./{name}\"\n"));
    for block in blocks(&std::fs::read_to_string(doc).unwrap()).iter().filter(|b| b.checked()) {
        if block.is_test() { tests.add(block) } else { program.add(block) }
    }
    let program_file = format!("{name}.grn");
    let tests_file = format!("{name}_test.grn");
    std::fs::write(dir.join(&program_file), &program.text).unwrap();

    let mut failures = Vec::new();
    let mut report = |what: &str, out: &Output| {
        let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let mut at: Vec<String> = lines_named(&all, &program_file)
            .into_iter()
            .filter_map(|l| program.block_at(l))
            .chain(lines_named(&all, &tests_file).into_iter().filter_map(|l| tests.block_at(l)))
            .map(|l| format!("the block at {shown}:{l}"))
            .collect();
        at.dedup();
        let place = if at.is_empty() { shown.clone() } else { at.join(", ") };
        failures.push(format!("`grenat {what}` fails on {place} (files in {}):\n{all}", dir.display()));
    };

    let out = grenat(&dir, &["check", &program_file]);
    if !out.status.success() {
        report("check", &out);
        return failures;
    }
    if !tests.spans.is_empty() {
        std::fs::write(dir.join(&tests_file), &tests.text).unwrap();
        let out = grenat(&dir, &["test", &tests_file]);
        if !out.status.success() {
            report("test", &out);
        }
    }
    failures
}

#[test]
fn every_ruby_block_of_the_references_checks_and_its_tests_pass() {
    let work = std::env::temp_dir().join(format!("grenat-llms-{}", std::process::id()));
    let failures: Vec<String> = documents().iter().flat_map(|doc| verify(doc, &work)).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn the_core_index_names_every_topic_file_and_only_those() {
    let core = std::fs::read_to_string(root().join("llms.txt")).unwrap();
    let index = &core[core.find("## Topics").expect("llms.txt has a `## Topics` index")..];
    let named: Vec<String> = index
        .lines()
        .filter_map(|l| l.strip_prefix("- `"))
        .filter_map(|l| l.split('`').next())
        .map(|t| format!("{t}.md"))
        .collect();
    let files: Vec<String> =
        documents().iter().skip(1).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    let mut sorted = named.clone();
    sorted.sort();
    assert_eq!(sorted, files, "the topics `llms.txt` names, and the files in docs/llms/");
}

#[test]
fn a_topic_file_says_when_to_read_it() {
    for doc in documents().iter().skip(1) {
        let text = std::fs::read_to_string(doc).unwrap();
        let shown = doc.file_name().unwrap().to_string_lossy();
        assert!(text.starts_with("# Grenat — "), "{shown} starts with its title");
        assert!(text.contains("Read this when"), "{shown} says when to read it");
        assert!(text.contains("llms.txt"), "{shown} says it assumes the core reference");
    }
}

#[test]
fn blocks_are_split_by_fence_and_kind() {
    let doc = "text\n```ruby\ndef f = 1\n```\n```sh\ngrenat run\n```\n```ruby skip-check\nnope\n```\n```ruby\ntest \"t\" do\nend\n```\n";
    let found = blocks(doc);
    assert_eq!(found.len(), 4);
    assert_eq!((found[0].line, found[0].text.as_str()), (3, "def f = 1\n"));
    assert!(found[0].checked() && !found[0].is_test());
    assert!(!found[1].checked() && !found[2].checked());
    assert!(found[3].checked() && found[3].is_test());

    let mut source = Source::new("a\nb\n");
    source.add(&found[0]);
    source.add(&found[3]);
    assert_eq!(source.block_at(4), Some(3));
    assert_eq!(source.block_at(6), Some(12));
    assert_eq!(source.block_at(1), None);
    assert_eq!(lines_named("error\n --> x.grn:12:3\n --> y.grn:1:1\n--> x.grn:4:1", "x.grn"), vec![12, 4]);
}
