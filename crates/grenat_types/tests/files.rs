//! Files in, checked: `on_file`'s pattern and options, its `fs.read`
//! effect (the directory of a literal pattern, any `fs.read` for a computed
//! one), the path its block receives — untrusted — and `deliver_file`, a
//! test double (E0500 elsewhere).

mod common;

use common::*;

#[test]
fn a_handler_gets_a_path() {
    clean(&format!(
        "{PRELUDE}prompt extract(invoice: Attachment) -> ~String using :fast
  user \"Extract.\", invoice
end
def size_of(path: ~String) -> Int = File.read(path).size
on_file \"inbox/*.pdf\", every: 30.s, settle: 2.s, existing: false do |path|
  puts extract(Pdf.read(path)).trust!
  puts size_of(path)
end
on_file \"drive/**/*.{{csv,CSV}}\", every: 10, settle: 0 do |path|
  puts File.lines(path).size
end
"
    ));
}

#[test]
fn the_path_is_untrusted() {
    let src = "on_file \"inbox/*.pdf\" do |path|\n  Shell.run([\"pdftotext\", path])\nend\n";
    single(src, "E0412", "[\"pdftotext\", path]");
    let src = "on_file \"inbox/*.pdf\" do |path|\n  File.write(\"done/\" + path, \"x\")\nend\n";
    single(src, "E0412", "\"done/\" + path");
    // checked, it may go anywhere
    clean(
        "on_file \"inbox/*.pdf\" do |path|\n  Shell.run([\"pdftotext\", path.check { |p| !p.include?(\"/-\") }?])\nend\n",
    );
}

#[test]
fn arguments_are_checked() {
    let d = single("on_file \"inbox/*\", every: \"often\" do |p|\nend\n", "E0200", "\"often\"");
    assert!(d.message.contains("`every:` a positive duration"), "{}", d.message);
    for every in ["0", "-1", "0.minutes"] {
        single(&format!("on_file \"inbox/*\", every: {every} do |p|\nend\n"), "E0200", every);
    }
    clean("on_file \"inbox/*\", settle: 0 do |p|\nend\n");
    single("on_file \"inbox/*\", settle: -1 do |p|\nend\n", "E0200", "-1");
    single("on_file \"inbox/*\", existing: 1 do |p|\nend\n", "E0200", "1");
    single("on_file \"inbox/*\", recursive: true do |p|\nend\n", "E0200", "true");
    single("on_file 42 do |p|\nend\n", "E0200", "42");
    single("on_file Credentials.fetch(:a, :b) do |p|\nend\n", "E0200", "Credentials.fetch(:a, :b)");
    single("on_file \"inbox/*\"\n", "E0200", "on_file \"inbox/*\"");
    single("on_file \"inbox/*\" do |p|\n  p.body\nend\n", "E0200", "body");
}

#[test]
fn a_literal_pattern_is_parsed_here() {
    for (pattern, why) in
        [("\"inbox/\"", "names a directory"), ("\"inbox/[ab\"", "never closed"), ("\"a/**\"", "ends with")]
    {
        let d = single(&format!("on_file {pattern} do |p|\nend\n"), "E0200", pattern);
        assert!(d.message.contains(why), "{}", d.message);
    }
}

#[test]
fn reading_a_folder_is_fs_read_on_its_directory() {
    let src = "def watch uses fs.read(\"outbox\")\n  on_file \"inbox/*.pdf\" do |p|\n  end\nend\n";
    let d = single(src, "E0300", "on_file \"inbox/*.pdf\" do |p|\n  end");
    assert_eq!(d.help.as_deref(), Some("add `fs.read(\"inbox\")` to `uses`"));
    for uses in ["fs.read(\"inbox\")", "fs.read(\"./inbox/\")", "fs.read", "fs"] {
        clean(&format!("def watch uses {uses}\n  on_file \"inbox/**/*.pdf\" do |p|\n  end\nend\n"));
    }
    clean("def watch uses fs.read(\"/srv\")\n  on_file \"/srv/drop/*.pdf\" do |p|\n  end\nend\n");
    single(
        "def watch uses net\n  on_file \"inbox/*\" do |p|\n  end\nend\n",
        "E0300",
        "on_file \"inbox/*\" do |p|\n  end",
    );
    // a computed pattern: any `fs.read` grant, the directory checked at run time
    clean("def watch(dir: String) uses fs.read(\"inbox\")\n  on_file dir + \"/*.pdf\" do |p|\n  end\nend\n");
    // declaring reads nothing: in a workflow, no `step`
    clean("workflow watch(n: Int) uses fs.read(\"inbox\")\n  on_file \"inbox/*\" do |p|\n  end\nend\n");
}

#[test]
fn an_untrusted_pattern_is_refused() {
    single(&format!("{PRELUDE}on_file summarize(\"x\").title do |p|\nend\n"), "E0412", "summarize(\"x\").title");
}

#[test]
fn deliver_file_exists_in_tests_only() {
    let handler = "on_file \"inbox/*\" do |p|\nend\n";
    let d = single(&format!("{handler}deliver_file(\"inbox/a\")\n"), "E0500", "deliver_file(\"inbox/a\")");
    assert!(d.message.contains("inside a test"), "{}", d.message);
    clean(&format!(
        "{handler}test \"in\" do\n  r = deliver_file(\"inbox/a\")\n  assert_equal \"handled\", r[\"status\"]\nend\n"
    ));
    let in_test = |call: &str| format!("{handler}test \"t\" do\n  {call}\nend\n");
    single(&in_test("deliver_file(3)"), "E0200", "3");
    single(&in_test("deliver_file(\"a\", \"b\")"), "E0200", "deliver_file(\"a\", \"b\")");
    single(&in_test("deliver_file(path: \"a\")"), "E0200", "deliver_file(path: \"a\")");
    single(&in_test("deliver_file(Credentials.fetch(:a, :b))"), "E0414", "Credentials.fetch(:a, :b)");
}
