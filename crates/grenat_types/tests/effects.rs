//! Declared and inferred effects (E0300, E0500).

mod common;

use common::*;

#[test]
fn main_must_declare_what_it_does() {
    let src = format!("{PRELUDE}def main\n  summarize(\"x\")\nend\n");
    let d = single(&src, "E0300", "summarize(\"x\")");
    assert_eq!(d.help.as_deref(), Some("add `uses llm`"));
    clean(&format!("{PRELUDE}def main uses llm\n  summarize(\"x\")\nend\n"));
}

#[test]
fn declared_effects_must_cover_the_body() {
    let src = "def load uses fs.read(\"./docs\")\n  File.read(\"./secrets/key\")\nend\n";
    let d = single(src, "E0300", "File.read(\"./secrets/key\")");
    assert!(d.message.contains("fs.read(\"./secrets/key\")"), "{}", d.message);
    clean("def load uses fs.read(\"./docs\")\n  File.read(\"./docs/a.md\")\nend\n");
    clean("def load uses fs\n  File.write(\"a\", \"b\")\nend\n");
}

#[test]
fn effects_propagate_through_undeclared_helpers() {
    let src = "def helper = File.read(\"a\")\ndef main uses llm\n  helper\nend\n";
    let d = single(src, "E0300", "helper");
    assert!(d.message.contains("fs.read(\"a\")"), "{}", d.message);
}

#[test]
fn tools_must_declare_effects() {
    let src = "tool read(path: String) -> String\n  File.read(path)\nend\n";
    single(src, "E0300", "File.read(path)");
}

#[test]
fn unknown_effect_names_are_reported() {
    let d = single("def f uses fs.raed\n  1\nend\n", "E0500", "fs.raed");
    assert_eq!(d.help.as_deref(), Some("did you mean `fs.read`?"));
}
