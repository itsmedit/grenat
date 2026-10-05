//! `Mail`: capabilities, taint, tests that never send, and a server that
//! fails on demand (`mock_mail`).

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
    let e = run_err(
        "Mail.connect(\"smtp://127.0.0.1:1\").send(from: \"a@b.c\", to: \"c@d.e\", subject: \"x\", body: \"y\")\n",
        Vec::new(),
    );
    assert_eq!(e.ty, "MailError");
}

#[test]
fn the_server_is_checked_when_the_mailer_is_made() {
    // no email needs to be sent: making the mailer is reaching for the server
    let e = run_err(
        "def mailer uses net(\"api.github.com\")\n  Mail.connect(\"smtps://bot:pw@smtp.acme.com:465\")\nend\nmailer\n",
        Vec::new(),
    );
    assert_eq!(e.ty, "CapabilityError");
    assert!(e.message.contains("`net` to `smtp://smtp.acme.com`"), "{}", e.message);
    assert!(!e.message.contains("pw"), "{}", e.message);
    run("def mailer uses net(\"smtp.acme.com\")\n  Mail.connect(\"smtp://smtp.acme.com\")\nend\nmailer\n");
    run("def mailer uses net\n  Mail.connect(\"smtp://smtp.acme.com\")\nend\nmailer\n");
    // a server named by a model's answer is not reached
    let src = format!("{SUMMARY}s = summarize(\"x\")\nMail.connect(\"smtp://#{{s.title}}\")\n");
    assert_eq!(run_err(&src, vec![summary_reply()]).ty, "TaintError");
}

const NOTIFY: &str = "\
def notify(subject: String) uses net(\"smtp.acme.com\")
  Mail.connect(\"smtp://smtp.acme.com\").send(from: \"bot@acme.com\", to: \"team@acme.com\", subject:, body: \"…\")
end
";

#[test]
fn mock_mail_makes_sending_fail() {
    tests_pass(&format!(
        "{NOTIFY}\
test \"the server is down, then back\" do
  notify(\"first\")
  mock_mail(raise: \"SMTP down\")
  e = assert_raises MailError do
    notify(\"lost\")
  end
  assert_equal \"SMTP down\", e.message
  assert_raises(MailError) {{ notify(\"lost again\") }}
  assert_equal [\"first\"], Mail.deliveries.map {{ |m| m[\"subject\"] }}
  mock_mail(raise: nil)
  notify(\"second\")
  assert_equal [\"first\", \"second\"], Mail.deliveries.map {{ |m| m[\"subject\"] }}
end

test \"each test starts with a working server\" do
  notify(\"fine\")
  assert_equal 1, Mail.deliveries.size
end
"
    ));
}

#[test]
fn a_crash_between_workflow_steps_resumes_after_them() {
    let output = tests_pass(&format!(
        "{NOTIFY}\
workflow publish(week: String) uses net(\"smtp.acme.com\")
  draft = step(:draft) do
    puts \"drafting\"
    \"Digest #{{week}}\"
  end
  step(:send) {{ notify(draft) }}
  draft
end

test \"the email fails, the run is resumed\" do
  mock_mail(raise: \"SMTP down\")
  assert_raises(MailError) {{ publish(\"w40\") }}
  assert_equal 0, Mail.deliveries.size
  mock_mail(raise: nil)
  assert_equal \"Digest w40\", publish(\"w40\")
  assert_equal [\"Digest w40\"], Mail.deliveries.map {{ |m| m[\"subject\"] }}
end
"
    ));
    // the finished step is replayed from the journal, not run again
    assert_eq!(output, "drafting\n");
}

#[test]
fn mock_mail_is_for_tests_and_takes_a_message() {
    let e = run_err("mock_mail(raise: \"SMTP down\")\n", Vec::new());
    assert_eq!(e.ty, "RuntimeError");
    assert!(e.message.contains("`mock_mail` only works in a test"), "{}", e.message);
    let (results, _) = test_outcomes(
        "\
test \"no reason\" do
  mock_mail
end

test \"another option\" do
  mock_mail(fail: \"x\")
end

test \"not a message\" do
  mock_mail(raise: 3)
end
",
    );
    let errors: Vec<&str> = results.iter().map(|(_, e)| e.as_deref().unwrap()).collect();
    assert_eq!(errors[0], "ArgumentError: `mock_mail` expects why sending fails: `mock_mail(raise: \"SMTP down\")`");
    assert_eq!(errors[1], errors[0]);
    assert_eq!(errors[2], "TypeError: `mock_mail` expects `raise:` a message, got 3");
}
