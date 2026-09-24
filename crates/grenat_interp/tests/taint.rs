//! Teinte `~T` à l'exécution : puits dangereux, validation, approbation humaine.

mod common;

use common::*;

#[test]
fn tainted_values_cannot_reach_dangerous_effects() {
    let src = format!("{SUMMARY}{MAILER}s = summarize(\"x\")\nsend(\"a@b.c\", \"Titre : #{{s.title}}\")\n");
    let e = run_err(&src, vec![summary_reply()]);
    assert_eq!(e.ty, "TaintError");
    assert!(e.message.contains("`send` (effet `net`)"), "{}", e.message);
}

#[test]
fn validated_values_can() {
    let src = format!(
        "{SUMMARY}{MAILER}s = summarize(\"x\").check {{ |x| x.bullets.size > 0 }}?\nsend(\"a@b.c\", s.title)\n"
    );
    let r = run_with(&src, vec![summary_reply()], &[]);
    r.result.unwrap();
    assert_eq!(r.output, "envoyé à a@b.c : Chat\n");
}

#[test]
fn failed_check_is_an_err_result() {
    let src = format!("{SUMMARY}r = summarize(\"x\").check {{ |x| x.bullets.size > 5 }}\np r.err?\nr?\n");
    let r = run_with(&src, vec![summary_reply()], &[]);
    assert_eq!(r.output, "true\n");
    assert_eq!(r.result.unwrap_err().ty, "CheckError");
}

#[test]
fn blocks_on_tainted_collections_see_tainted_items() {
    let src = format!("{SUMMARY}s = summarize(\"x\")\ns.bullets.each {{ |b| p b.tainted? }}\n");
    let r = run_with(&src, vec![summary_reply()], &[]);
    r.result.unwrap();
    assert_eq!(r.output, "true\n");
}

#[test]
fn human_approval_untaints_or_denies() {
    let src = format!("{SUMMARY}{MAILER}s = summarize(\"x\").approve(by: :human)\nsend(\"a@b.c\", s.title)\n");
    let approved = run_with(&src, vec![summary_reply()], &["o"]);
    approved.result.unwrap();
    assert!(approved.output.ends_with("envoyé à a@b.c : Chat\n"), "{}", approved.output);

    let denied = run_with(&src, vec![summary_reply()], &["n"]);
    assert_eq!(denied.result.unwrap_err().ty, "ApprovalDenied");
}
