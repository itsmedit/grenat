//! The `~T` taint (E0412, E0413).

mod common;

use common::*;

#[test]
fn llm_output_cannot_reach_the_network() {
    let src = format!("{PRELUDE}s = summarize(\"x\")\nsend(\"a@b.c\", s.title)\n");
    let d = single(&src, "E0412", "s.title");
    assert!(d.message.contains("`send` (effect `net`)"), "{}", d.message);
    assert_eq!(&src[d.notes[0].0.range()], "summarize(\"x\")");
}

#[test]
fn validation_clears_the_taint() {
    for fix in [
        "s = summarize(\"x\").check { |x| x.bullets.size > 0 }?",
        "s = summarize(\"x\").trust!",
        "s = summarize(\"x\").approve(by: :human)",
    ] {
        clean(&format!("{PRELUDE}{fix}\nsend(\"a@b.c\", s.title)\n"));
    }
}

#[test]
fn taint_flows_through_interpolation_blocks_and_collections() {
    for leak in [
        "send(\"a\", \"Summary: #{s.title}\")",
        "s.bullets.each { |b| send(\"a\", b) }",
        "xs = []\nxs << s.title\nsend(\"a\", xs.join(\", \"))",
        "send(\"a\", s.headline)",
        "t = s.title\nu = t.upcase\nsend(\"a\", u)",
    ] {
        let src = format!("{PRELUDE}s = summarize(\"x\")\n{leak}\n");
        let d = diags(&src);
        assert!(d.iter().any(|d| d.code == Some("E0412")), "leak not detected:\n{leak}");
    }
}

#[test]
fn taint_flows_through_helper_functions() {
    // the intermediate function is checked for the actual taint of its argument
    let src = format!(
        "{PRELUDE}def format(s: Summary) -> String = \"#{{s.title}} !\"\nsend(\"a\", format(summarize(\"x\")))\n"
    );
    single(&src, "E0412", "format(summarize(\"x\"))");
    // … and the same function remains usable with a clean value
    clean(&format!(
        "{PRELUDE}def format(s: Summary) -> String = \"#{{s.title}} !\"\nsend(\"a\", format(Summary(title: \"t\", bullets: [])))\n"
    ));
}

#[test]
fn taint_is_tracked_in_agent_state() {
    let src = format!(
        "{PRELUDE}agent Notes
  @last: String = \"\"
  on Remember(text: String)
    @last = summarize(text).title
  end
  on Publish
    send(\"a\", @last)
  end
end
"
    );
    single(&src, "E0412", "@last");
}

#[test]
fn file_write_is_a_dangerous_sink() {
    let src = format!("{PRELUDE}def main uses llm, fs.write\n  File.write(\"out.txt\", summarize(\"x\").title)\nend\n");
    single(&src, "E0412", "summarize(\"x\").title");
}

#[test]
fn prompts_and_run_must_declare_taint() {
    let src = "model :m, name: \"claude-haiku-4-5\"\nprompt p(x: String) -> String\n  user x\nend\n";
    let d = single(src, "E0413", "String");
    assert_eq!(d.help.as_deref(), Some("write `-> ~String`"));

    let src = "model :m, name: \"claude-haiku-4-5\"\nagent A\n  on Go -> String\n    run \"fais\"\n  end\nend\n";
    single(src, "E0413", "String");
}
