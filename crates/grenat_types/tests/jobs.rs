//! Jobs: `enqueue` names a function of the program and gives it arguments
//! it takes, positional or keyword (E0100, E0200).

mod common;

use common::*;

const WEEKLY: &str = "workflow weekly(monday: String, repos: Array(String) = []) -> String\n  monday\nend\n";

fn queue(call: &str) -> String {
    format!("{WEEKLY}def go() uses db\n  {call}\nend\n")
}

#[test]
fn a_job_takes_positional_and_keyword_arguments() {
    clean(&queue("enqueue(:weekly, \"2026-09-28\")"));
    clean(&queue("enqueue(:weekly, monday: \"2026-09-28\")"));
    clean(&queue("enqueue(:weekly, monday: \"2026-09-28\", repos: [\"a/b\"], in: 1.hour)"));
    clean(&queue("enqueue(:weekly, \"2026-09-28\", repos: [\"a/b\"])"));
}

#[test]
fn a_job_binds_to_its_function() {
    let d = diags(&queue("enqueue(:weekly, mondy: \"2026-09-28\")"));
    assert!(d.iter().any(|d| d.message.contains("unknown named argument `mondy:` for `weekly`")), "{d:#?}");
    assert!(d.iter().all(|d| d.code == Some("E0200")), "{d:#?}");
    single(&queue("enqueue(:weekly, 3)"), "E0200", "3");
    single(&queue("enqueue(:weekly, \"a\", [], 4)"), "E0200", "4");
    let d = diags(&queue("enqueue(:weekly)"));
    assert_eq!(d.len(), 1, "{d:#?}");
    assert!(d[0].message.contains("missing argument `monday` for `weekly`"), "{d:#?}");
}

#[test]
fn a_job_names_a_function_of_the_program() {
    let d = single(&queue("enqueue(:weeklly, \"x\")"), "E0100", ":weeklly");
    assert!(d.message.contains("`enqueue`: unknown function `weeklly`"), "{d:#?}");
    // computed: checked when the job is queued
    clean(&format!("{WEEKLY}def go(name: Symbol) uses db\n  enqueue(name, \"x\")\nend\n"));
}

#[test]
fn a_keyword_of_a_job_is_never_untrusted() {
    single(
        &format!("{WEEKLY}post \"/w\" do |req|\n  enqueue(:weekly, monday: req.params[\"m\"])\nend\n"),
        "E0412",
        "req.params[\"m\"]",
    );
}
