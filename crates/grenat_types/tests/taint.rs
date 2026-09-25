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

#[test]
fn what_the_network_returns_is_untrusted() {
    let head = "def main uses net\n  r = Http.get(\"https://x.io/a\")\n";
    single(&format!("{head}  Http.post(\"https://x.io/b\", body: r.body)\nend\n"), "E0412", "r.body");
    single(&format!("{head}  Http.post(\"https://x.io/b\", json: Json.parse(r.body))\nend\n"), "E0412", "Json.parse(r.body)");
    single(&format!("{head}  Http.post(\"https://x.io/b\", json: r.json)\nend\n"), "E0412", "r.json");
    clean(&format!("{head}  Http.post(\"https://x.io/b\", json: {{code: r.status}})\nend\n"));
    clean(&format!("{head}  Http.post(\"https://x.io/b\", body: r.body.check {{ |b| b.size < 100 }}?)\nend\n"));
}

#[test]
fn a_model_s_answer_cannot_be_sent_unchecked() {
    let src = format!("{PRELUDE}def main uses llm, net\n  s = summarize(\"x\")\n  Http.post(\"https://x.io\", json: {{t: s.title}})\nend\n");
    let d = single(&src, "E0412", "{t: s.title}");
    assert!(d.message.starts_with("an untrusted value reaches `Http.post` (effect `net`)"), "{}", d.message);
}

#[test]
fn sql_is_never_untrusted() {
    let head = format!("{PRELUDE}def main uses llm, db\n  db = Db.connect(\"sqlite::memory:\")\n  s = summarize(\"x\")\n");
    single(&format!("{head}  db.query(s.title)\nend\n"), "E0412", "s.title");
    clean(&format!("{head}  db.query(\"SELECT * FROM t WHERE title = ?\", [s.title])\nend\n"));
    single(&format!("{head}  db.execute(\"UPDATE t SET title = ?\", [s.title])\nend\n"), "E0412", "[s.title]");
}

#[test]
fn what_a_program_prints_is_untrusted() {
    let head = "def main uses shell\n  out = Shell.run([\"git\", \"log\"]).stdout\n";
    single(&format!("{head}  Shell.run([\"echo\", out])\nend\n"), "E0412", "[\"echo\", out]");
    clean(&format!("{head}  Shell.run([\"echo\", out.check {{ |o| o.size < 9 }}?])\nend\n"));
}

#[test]
fn what_an_mcp_server_answers_is_untrusted() {
    let head = "def main uses mcp, net\n  issues = Mcp.call(:linear, \"list_issues\")\n";
    single(&format!("{head}  Http.post(\"https://x.io\", body: issues)\nend\n"), "E0412", "issues");
    clean(&format!("{head}  Http.post(\"https://x.io\", body: issues.check {{ |i| i.size < 99 }}?)\nend\n"));
}

#[test]
fn what_a_webhook_carries_is_untrusted() {
    single("on_webhook \"/x\" do |req|\n  Shell.run([\"echo\", req.body])\nend\n", "E0412", "[\"echo\", req.body]");
    clean("on_webhook \"/x\" do |req|\n  p req.path\n  Shell.run([\"echo\", req.path])\nend\nevery cron: \"0 8 * * MON\" do\n  p 1\nend\n");
}

#[test]
fn an_email_is_never_untrusted() {
    let head = format!("{PRELUDE}def main uses llm, net\n  s = summarize(\"x\")\n  m = Mail.connect(\"smtp://smtp.acme.com\")\n");
    single(&format!("{head}  m.send(from: \"a@b.c\", to: \"c@d.e\", subject: \"x\", body: s.title)\nend\n"), "E0412", "s.title");
    clean(&format!("{head}  m.send(from: \"a@b.c\", to: \"c@d.e\", subject: \"x\", body: s.trust!.title)\nend\n"));
}
