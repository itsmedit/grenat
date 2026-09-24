//! Alignement de l'exécution sur le vérificateur : self teinté, validation, capacités disque.

mod common;

use common::*;

#[test]
fn methods_on_tainted_values_keep_self_tainted() {
    let src = format!(
        "{SUMMARY}{MAILER}struct Box2\n  inner: Summary\n  def mail = send(\"a\", inner.title)\nend\nb = Box2(inner: summarize(\"x\"))\nb.mail\n"
    );
    let e = run_err(&src, vec![summary_reply()]);
    assert_eq!(e.ty, "TaintError");
}

#[test]
fn validation_methods_accept_clean_values() {
    let src = "p \"a\".trust!, [1].check { |x| x.size > 0 }.ok?\n";
    assert_eq!(run(src), "\"a\"\ntrue\n");
}

#[test]
fn filesystem_capabilities_are_enforced() {
    let src = "def load(path: String) -> String uses fs.read(\"./src\")\n  File.read(path)\nend\nputs load(\"src/lib.rs\").lines.first\nload(\"Cargo.toml\")\n";
    let r = run_with(src, vec![], &[]);
    assert!(r.output.starts_with("//! Interpréteur"), "{}", r.output);
    let e = r.result.unwrap_err();
    assert_eq!(e.ty, "CapabilityError");
    assert!(e.message.contains("`load` (uses fs.read(\"./src\"))"), "{}", e.message);

    let escape = "def load(path: String) -> String uses fs.read(\"./src\")\n  File.read(path)\nend\nload(\"src/../Cargo.toml\")\n";
    assert_eq!(run_err(escape, vec![]).ty, "CapabilityError");
}
