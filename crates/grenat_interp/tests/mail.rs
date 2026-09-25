//! `Mail`: capabilities, taint, and tests that never send.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, run_tests};

#[test]
fn tests_keep_what_would_be_sent() {
    let src = "\
def notify(to: String) uses net(\"smtp.acme.com\")
  mailer = Mail.connect(\"smtps://bot:pw@smtp.acme.com:465\")
  mailer.send(from: \"bot@acme.com\", to: to, subject: \"Hi\", body: \"Hello\")
end
test \"one email\" do
  notify(\"ada@acme.com\")
  sent = Mail.deliveries
  assert_equal 1, sent.size
  assert_equal [\"ada@acme.com\"], sent.first[\"to\"]
  assert_equal \"Hi\", sent.first[\"subject\"]
end
test \"a test starts with none\" do
  assert_equal 0, Mail.deliveries.size
end
test \"missing fields\" do
  Mail.connect(\"smtp://smtp.acme.com\").send(to: \"a@b.c\", subject: \"x\")
end
";
    let parsed = grenat_parser::parse(src);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    let outcomes = run_tests(&parsed.program, options).unwrap();
    assert!(outcomes[0].error.is_none(), "{:?}", outcomes[0].error);
    assert!(outcomes[1].error.is_none(), "{:?}", outcomes[1].error);
    assert_eq!(outcomes[2].error.as_ref().unwrap().message, "`send` expects `from:`, `to:`, `subject:` and `body:`");
}

#[test]
fn the_server_is_a_capability_and_nothing_untrusted_is_sent() {
    let e = run_err(
        "def notify uses net(\"api.github.com\")\n  Mail.connect(\"smtp://smtp.acme.com\").send(from: \"a@b.c\", to: \"c@d.e\", subject: \"x\", body: \"y\")\nend\nnotify\n",
        Vec::new(),
    );
    assert_eq!(e.ty, "CapabilityError");
    let src = format!(
        "{SUMMARY}s = summarize(\"x\")\nMail.connect(\"smtp://smtp.acme.com\").send(from: \"a@b.c\", to: \"c@d.e\", subject: \"x\", body: s.title)\n"
    );
    assert_eq!(run_err(&src, vec![summary_reply()]).ty, "TaintError");
    let e = run_err("Mail.connect(\"smtp.acme.com\")\n", Vec::new());
    assert_eq!(e.ty, "ArgumentError");
    // a real send to a server that is not there fails cleanly
    let e = run_err("Mail.connect(\"smtp://127.0.0.1:1\").send(from: \"a@b.c\", to: \"c@d.e\", subject: \"x\", body: \"y\")\n", Vec::new());
    assert_eq!(e.ty, "MailError");
}
