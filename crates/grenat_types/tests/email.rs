//! Email in, checked: `on_email`'s arguments, its `net` effect (the host
//! of a literal URL, any `net` for a configured one), the `IncomingEmail`
//! its block receives — every field untrusted — and `deliver_email`, a
//! test double (E0500 elsewhere).

mod common;

use common::*;

const URL: &str = "\"imaps://support%40acme.com:pw@imap.gmail.com/INBOX\"";

#[test]
fn a_handler_reads_typed_fields() {
    clean(&format!(
        "{PRELUDE}prompt reply(subject: String, body: String, documents: Array(Attachment)) -> ~String using :fast
  user subject, body
end
def triage(email: IncomingEmail) -> Int = email.to.size + email.cc.size
def sender(email: IncomingEmail) -> ~String? = email.from
def sent_at(email: IncomingEmail) -> ~Float? = email.date
def html(email: IncomingEmail) -> ~String? = email.html
def kinds(email: IncomingEmail) -> ~Array(String) = email.attachments.map {{ |a| a.kind + a.media_type + (a.name || \"\") }}
on_email {URL}, every: 1.minute, move_to: \"Done\" do |email|
  puts reply(email.subject, email.text, email.attachments).trust!
  puts triage(email)
end
on_email Credentials.fetch(:support, :imap_url), every: 30 do |email|
  puts email.message_id
end
"
    ));
}

#[test]
fn arguments_are_checked() {
    let d = single(&format!("on_email {URL}, every: \"often\" do |e|\nend\n"), "E0200", "\"often\"");
    assert!(d.message.contains("`every:` a duration"), "{}", d.message);
    single(&format!("on_email {URL}, colour: 1 do |e|\nend\n"), "E0200", "1");
    single(&format!("on_email {URL}, move_to: 3 do |e|\nend\n"), "E0200", "3");
    single(&format!("on_email {URL}, token: \"t\" do |e|\nend\n"), "E0200", "\"t\"");
    single("on_email 42 do |e|\nend\n", "E0200", "42");
    single(&format!("on_email {URL}\n"), "E0200", &format!("on_email {URL}"));
    single(&format!("on_email {URL} do |e|\n  e.body\nend\n"), "E0200", "body");
    clean(&format!("def token -> String = \"t\"\non_email {URL}, token: :token do |e|\nend\n"));
}

#[test]
fn reading_a_mailbox_is_net_on_its_host() {
    let src = format!("def watch uses net(\"api.github.com\")\n  on_email {URL} do |e|\n  end\nend\n");
    let d = single(&src, "E0300", &format!("on_email {URL} do |e|\n  end"));
    assert_eq!(d.help.as_deref(), Some("add `net(\"imap.gmail.com\")` to `uses`"));
    for uses in ["net(\"imap.gmail.com\")", "net"] {
        clean(&format!("def watch uses {uses}\n  on_email {URL} do |e|\n  end\nend\n"));
    }
    // a configured URL: any `net` grant, the host checked at run time
    for uses in ["net", "net(\"imap.gmail.com\")"] {
        clean(&format!(
            "def watch uses {uses}, env\n  on_email Credentials.fetch(:support, :imap_url) do |e|\n  end\nend\n"
        ));
    }
    single(
        "def watch uses env\n  on_email Env.fetch(\"IMAP_URL\") do |e|\n  end\nend\n",
        "E0300",
        "on_email Env.fetch(\"IMAP_URL\") do |e|\n  end",
    );
    // declaring opens nothing: in a workflow, no `step`
    clean(&format!("workflow watch(n: Int) uses net(\"imap.gmail.com\")\n  on_email {URL} do |e|\n  end\nend\n"));
}

#[test]
fn every_field_is_untrusted() {
    for field in [
        "from.to_s",
        "from_name.to_s",
        "to.first",
        "cc.first",
        "reply_to.to_s",
        "subject",
        "date.to_s",
        "message_id.to_s",
        "text",
        "html.to_s",
        "attachments.first.media_type",
        "attachments.first.name.to_s",
    ] {
        let src = format!("on_email {URL} do |email|\n  Shell.run([\"echo\", email.{field}])\nend\n");
        single(&src, "E0412", &format!("[\"echo\", email.{field}]"));
    }
    // checked, it may go anywhere
    clean(&format!(
        "on_email {URL} do |email|\n  Shell.run([\"echo\", email.subject.check {{ |s| s.size < 80 }}?])\nend\n"
    ));
}

#[test]
fn an_untrusted_url_is_refused() {
    let src = format!("{PRELUDE}on_email summarize(\"x\").title do |e|\nend\n");
    single(&src, "E0412", "summarize(\"x\").title");
}

#[test]
fn incoming_email_is_built_in() {
    let d = single("struct IncomingEmail\n  from: String\nend\n", "E0100", "IncomingEmail");
    assert!(d.message.contains("built in"), "{}", d.message);
}

#[test]
fn deliver_email_exists_in_tests_only() {
    let handler = format!("on_email {URL} do |e|\nend\n");
    let d = single(&format!("{handler}deliver_email(from: \"a@b.c\")\n"), "E0500", "deliver_email(from: \"a@b.c\")");
    assert!(d.message.contains("inside a test"), "{}", d.message);
    single(
        &format!("{handler}def helper = deliver_email(from: \"a@b.c\")\n"),
        "E0500",
        "deliver_email(from: \"a@b.c\")",
    );
    clean(&format!(
        "{handler}test \"in\" do
  r = deliver_email(from: \"a@b.c\", to: [\"x@y.z\"], cc: \"c@d.e\", subject: \"s\", text: \"t\", html: \"<p>t</p>\", date: 1.5, attachments: [Pdf.read(\"a.pdf\")], folder: \"INBOX\")
  assert_equal \"seen\", r[\"status\"]
end
"
    ));
}

#[test]
fn deliver_email_fields_are_typed() {
    let in_test = |call: &str| format!("on_email {URL} do |e|\nend\ntest \"t\" do\n  {call}\nend\n");
    single(&in_test("deliver_email(from: \"a@b.c\", subject: 3)"), "E0200", "3");
    single(&in_test("deliver_email(from: \"a@b.c\", to: [1])"), "E0200", "[1]");
    single(&in_test("deliver_email(from: \"a@b.c\", attachments: [\"a.pdf\"])"), "E0200", "[\"a.pdf\"]");
    single(&in_test("deliver_email(from: \"a@b.c\", body: \"x\")"), "E0200", "\"x\"");
    single(&in_test("deliver_email(subject: \"x\")"), "E0200", "deliver_email(subject: \"x\")");
    single(&in_test("deliver_email(\"a@b.c\")"), "E0200", "\"a@b.c\"");
    let d = single(
        &in_test("deliver_email(from: \"a@b.c\", text: Credentials.fetch(:a, :b))"),
        "E0414",
        "Credentials.fetch(:a, :b)",
    );
    assert!(d.message.contains("`deliver_email` expects a `String` for `text`"), "{}", d.message);
}
