//! Email in (`on_email`): the declaration's checks, `deliver_email` in
//! tests (fields, outcomes, taint, attachments into a prompt), and
//! `grenat serve` against an IMAP server in process — a message handled
//! once then seen or moved, a raising handler retried then flagged, a
//! dropped connection, a password never shown.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use grenat_imap::fake::{Config, FakeImap, Mode, PASSWORD};
use grenat_interp::{Options, Output, Response, Scripted, run_main, run_tests};

const URL: &str = "imaps://support%40acme.com:pw@imap.acme.com/INBOX";

/// A file's tests, each with its error (`None` when it passed).
fn test_results(
    src: &str,
    provider: Option<Arc<Scripted>>,
    dir: Option<std::path::PathBuf>,
) -> Vec<(String, Option<String>)> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        provider: provider.map(|p| p as Arc<dyn grenat_interp::Provider>),
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        dir,
        ..Options::default()
    };
    run_tests(&parsed.program, options)
        .unwrap()
        .into_iter()
        .map(|o| (o.name, o.error.map(|e| format!("{}: {}", e.ty, e.message))))
        .collect()
}

fn all_pass(src: &str) {
    for (name, error) in test_results(src, None, None) {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
}

#[test]
fn deliver_email_hands_the_handler_a_message() {
    all_pass(&format!(
        "on_email \"{URL}\" do |email|
  assert_equal \"ada@acme.com\", email.from
  assert_equal \"Ada\", email.from_name
  assert_equal [\"support@acme.com\", \"sales@acme.com\"], email.to
  assert_equal [\"boss@acme.com\"], email.cc
  assert_equal \"Refund\", email.subject
  assert_equal \"Please refund.\", email.text
  assert_equal \"2026-10-07T08:00:00Z\", Time.iso(email.date.trust!)
  assert_equal \"m1@acme.com\", email.message_id
  assert_equal nil, email.html
  assert_equal 0, email.attachments.size
end
test \"every field\" do
  r = deliver_email(
    from: \"ada@acme.com\",
    from_name: \"Ada\",
    to: [\"support@acme.com\", \"sales@acme.com\"],
    cc: \"boss@acme.com\",
    subject: \"Refund\",
    text: \"Please refund.\",
    date: \"2026-10-07T08:00:00Z\",
    message_id: \"m1@acme.com\",
  )
  assert_equal({{\"status\" => \"seen\"}}, r)
end
"
    ));
}

#[test]
fn a_moved_message_and_a_raising_handler() {
    all_pass(&format!(
        "on_email \"{URL}\", every: 30.s, move_to: \"Done\" do |email|
  raise ArgumentError, \"no text\" if email.text.empty?
end
test \"moved\" do
  assert_equal({{\"status\" => \"moved\", \"folder\" => \"Done\"}}, deliver_email(from: \"a@b.c\", text: \"hi\"))
end
test \"failed\" do
  r = deliver_email(from: \"a@b.c\", subject: \"empty\")
  assert_equal \"failed\", r[\"status\"]
  assert_equal \"ArgumentError: no text\", r[\"error\"]
end
test \"html only reads as text too\" do
  assert_equal \"moved\", deliver_email(from: \"a@b.c\", html: \"<p>hi</p>\")[\"status\"]
end
"
    ));
}

#[test]
fn every_field_is_untrusted_at_run_time() {
    all_pass(&format!(
        "on_email \"{URL}\" do |email|
  [email.from, email.subject, email.text, email.to, email.to.first, email.date, email.attachments].each do |v|
    assert v.tainted?
  end
  Shell.run([\"echo\", email.subject])
end
test \"nothing from an email reaches a command unchecked\" do
  r = deliver_email(from: \"a@b.c\", subject: \"; rm -rf /\", to: \"x@y.z\")
  assert_equal \"failed\", r[\"status\"]
  assert r[\"error\"].start_with?(\"TaintError\")
end
"
    ));
}

#[test]
fn attachments_reach_a_prompt() {
    let dir = temp_dir("email-attachments");
    std::fs::write(dir.join("invoice.pdf"), b"%PDF-1.4 x").unwrap();
    let src = format!(
        "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt summarize(text: String, document: Attachment) -> ~String using :fast
  user \"Summarize.\", document, text
end
on_email \"{URL}\" do |email|
  invoice = email.attachments.first
  assert_equal \"invoice.pdf\", invoice.name
  assert_equal \"document\", invoice.kind
  assert_equal \"application/pdf\", invoice.media_type
  puts summarize(email.text, invoice).trust!
end
test \"the invoice goes to the model\" do
  assert_equal \"seen\", deliver_email(from: \"a@b.c\", text: \"See attached.\", attachments: [Pdf.read(\"{}/invoice.pdf\")])[\"status\"]
end
",
        dir.display()
    );
    let provider = Arc::new(Scripted::new(vec![Response::text_reply("an invoice")]));
    for (name, error) in test_results(&src, Some(provider.clone()), Some(dir)) {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
    let requests = provider.requests();
    let content = &requests[0]["messages"][0]["content"];
    assert_eq!(content[0]["type"], "document");
    assert_eq!(content[0]["source"]["media_type"], "application/pdf");
    assert_eq!(content[0]["source"]["data"], "JVBERi0xLjQgeA==");
    assert_eq!(content[1]["text"], "Summarize.\nSee attached.");
}

#[test]
fn deliver_email_is_a_test_double_with_checked_arguments() {
    let e = run_err(&format!("on_email \"{URL}\" do |e|\nend\ndeliver_email(from: \"a@b.c\")\n"), Vec::new());
    assert_eq!(e.ty, "RuntimeError");
    assert!(e.message.contains("only works in a test"), "{}", e.message);
    let two = "on_email \"imaps://u:p@imap.acme.com/INBOX\" do |e|\n  puts \"inbox\"\nend\non_email \"imaps://u:p@imap.acme.com/Billing\" do |e|\n  raise \"billing\"\nend\n";
    let src = format!(
        "{two}test \"which mailbox\" do\n  deliver_email(from: \"a@b.c\")\nend
test \"by folder\" do\n  assert_equal \"failed\", deliver_email(from: \"a@b.c\", folder: \"Billing\")[\"status\"]\n  assert_equal \"seen\", deliver_email(from: \"a@b.c\", folder: \"INBOX\")[\"status\"]\nend
test \"no such folder\" do\n  deliver_email(from: \"a@b.c\", folder: \"Spam\")\nend
test \"an unknown field\" do\n  deliver_email(from: \"a@b.c\", folder: \"INBOX\", body: \"x\")\nend
test \"no sender\" do\n  deliver_email(folder: \"INBOX\", subject: \"x\")\nend
test \"a secret\" do\n  deliver_email(from: \"a@b.c\", folder: \"INBOX\", text: Credentials.fetch(:x, :y))\nend
test \"not an attachment\" do\n  deliver_email(from: \"a@b.c\", folder: \"INBOX\", attachments: [\"a.pdf\"])\nend
"
    );
    let results: Vec<Option<String>> = test_results(&src, None, None).into_iter().map(|(_, e)| e).collect();
    let error = |i: usize| results[i].clone().unwrap_or_default();
    assert!(error(0).starts_with("ArgumentError: several mailboxes"), "{}", error(0));
    assert_eq!(results[1], None);
    assert!(error(2).contains("no `on_email` reads the folder \"Spam\""), "{}", error(2));
    assert!(error(3).contains("no field `body:`"), "{}", error(3));
    assert!(error(4).contains("expects `from:`"), "{}", error(4));
    assert!(error(5).starts_with("SecretError"), "{}", error(5));
    assert!(error(6).starts_with("TypeError"), "{}", error(6));
    let none = "test \"no mailbox\" do\n  deliver_email(from: \"a@b.c\")\nend\n";
    assert!(test_results(none, None, None)[0].1.as_ref().unwrap().contains("no mailbox to deliver to"));
}

#[test]
fn the_declaration_checks_its_url_its_host_and_its_options() {
    // the host is held to the capabilities of the function declaring it
    let e = run_err(
        &format!("def watch uses net(\"api.github.com\")\n  on_email \"{URL}\" do |e|\n  end\nend\nwatch\n"),
        Vec::new(),
    );
    assert_eq!(e.ty, "CapabilityError");
    assert!(e.message.contains("imap.acme.com") && !e.message.contains("pw"), "{}", e.message);
    run(&format!("def watch uses net(\"imap.acme.com\")\n  on_email \"{URL}\" do |e|\n  end\nend\nwatch\n"));
    // no password in what an invalid URL says
    let e = run_err("on_email \"https://u:hunter2@imap.acme.com\" do |e|\nend\n", Vec::new());
    assert_eq!(e.ty, "ArgumentError");
    assert!(!e.message.contains("hunter2"), "{}", e.message);
    for (option, error) in [
        ("every: 0", "ArgumentError"),
        ("move_to: \"\"", "ArgumentError"),
        ("token: :nope", "NameError"),
        ("token: :needs_one", "ArgumentError"),
        ("tls: false", "ArgumentError"),
    ] {
        let src = format!("def needs_one(x: Int) -> String = \"t\"\non_email \"{URL}\", {option} do |e|\nend\n");
        let e = run_err(&src, Vec::new());
        assert_eq!(e.ty, error, "{option}: {}", e.message);
    }
    let src = format!("{SUMMARY}s = summarize(\"x\")\non_email s.title do |e|\nend\n");
    assert_eq!(run_err(&src, vec![summary_reply()]).ty, "TaintError");
    // in tests, a stand-in credential names no mailbox: nothing is read
    all_pass(
        "on_email Credentials.fetch(:support, :imap_url) do |email|\n  puts email.subject\nend\ntest \"stand-in\" do\n  assert_equal \"seen\", deliver_email(from: \"a@b.c\")[\"status\"]\nend\n",
    );
}

// ── Served, against an IMAP server in process ─────────────────────────

fn message(subject: &str) -> Vec<u8> {
    format!("From: Ada <ada@acme.com>\r\nTo: support@acme.com\r\nSubject: {subject}\r\nMessage-ID: <{subject}@acme.com>\r\n\r\nBody of {subject}.\r\n").into_bytes()
}

/// Serves `src` on a thread of its own (it never returns), trusting the
/// fake server's authority; what it prints and logs.
fn serve(src: &str, server: &FakeImap, credentials_root: Option<std::path::PathBuf>) -> Arc<Mutex<String>> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let program: &'static grenat_ast::Program = Box::leak(Box::new(parsed.program));
    let buffer = Arc::new(Mutex::new(String::new()));
    let options = Options {
        output: Output::Capture(buffer.clone()),
        journal: Some(temp_dir("journal")),
        trusted_certificates: vec![server.authority()],
        credentials_root,
        ..Options::default()
    };
    std::thread::spawn(move || grenat_interp::serve(program, options, "127.0.0.1:0", |_| {}));
    buffer
}

/// Waits (20 s at most) until `done` holds.
fn wait_until(what: &str, output: &Arc<Mutex<String>>, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(20), "timed out waiting: {what}\n{}", output.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn seen(server: &FakeImap, folder: &str) -> usize {
    server.messages(folder).iter().filter(|m| m.has("\\Seen")).count()
}

#[test]
fn served_a_message_is_handled_once_then_marked_seen() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.deliver("INBOX", &message("first"));
    server.deliver_with("INBOX", &message("already read"), &["\\Seen"]);
    server.deliver("INBOX", &message("second"));
    let url = server.url("imaps", "INBOX");
    let output = serve(
        &format!(
            "on_email \"{url}\", every: 0.1 do |email|\n  puts \"handled #{{email.subject.trust!}} from #{{email.from.trust!}}\"\nend\n"
        ),
        &server,
        None,
    );
    wait_until("both seen", &output, || seen(&server, "INBOX") == 3);
    server.deliver("INBOX", &message("third"));
    wait_until("the third seen", &output, || seen(&server, "INBOX") == 4);
    // a few more reads: nothing is handled twice
    std::thread::sleep(Duration::from_millis(400));
    let out = output.lock().unwrap().clone();
    for subject in ["first", "second", "third"] {
        assert_eq!(out.matches(&format!("handled {subject} from ada@acme.com\n")).count(), 1, "{out}");
    }
    assert!(!out.contains("already read"), "{out}");
    assert!(out.contains("[imap] 127.0.0.1/INBOX: 2 new\n"), "{out}");
    assert!(!out.contains(PASSWORD) && !out.contains("support%40acme.com"), "{out}");
    // read with BODY.PEEK[], then marked seen once handled
    let commands: Vec<String> = server.commands().into_iter().map(|(_, c)| c).collect();
    assert!(commands.iter().any(|c| c.contains("BODY.PEEK[]")), "{commands:?}");
    assert!(commands.iter().any(|c| c.starts_with("UID STORE 1 +FLAGS")), "{commands:?}");
}

#[test]
fn served_handled_messages_are_moved() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.create("Done");
    server.deliver("INBOX", &message("refund"));
    let url = server.url("imaps", "INBOX");
    let output = serve(
        &format!("on_email \"{url}\", every: 0.1, move_to: \"Done\" do |email|\n  puts email.subject.trust!\nend\n"),
        &server,
        None,
    );
    wait_until("moved", &output, || server.messages("Done").len() == 1);
    assert!(server.messages("INBOX").is_empty());
    assert_eq!(output.lock().unwrap().matches("refund\n").count(), 1);
}

#[test]
fn served_a_raising_handler_is_retried_then_flagged_and_the_server_goes_on() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.deliver("INBOX", &message("poison"));
    server.deliver("INBOX", &message("fine"));
    let (path, db) = {
        let path = temp_dir("email-events").join("app.db");
        let url = format!("sqlite://{}", path.display());
        (path, url)
    };
    let url = server.url("imaps", "INBOX");
    let src = format!(
        "database \"{db}\"
on_email \"{url}\", every: 0.1 do |email|
  raise ArgumentError, \"cannot handle it\" if email.subject == \"poison\"
  puts \"handled #{{email.subject.trust!}}\"
end
"
    );
    let output = serve(&src, &server, None);
    let flagged = || server.messages("INBOX").iter().any(|m| m.uid == 1 && m.has("\\Flagged") && m.has("\\Seen"));
    wait_until("the poison flagged", &output, flagged);
    wait_until("the fine one seen", &output, || seen(&server, "INBOX") == 2);
    std::thread::sleep(Duration::from_millis(300));
    let out = output.lock().unwrap().clone();
    assert_eq!(out.matches("handled fine\n").count(), 1, "{out}");
    assert_eq!(out.matches("[imap] 127.0.0.1/INBOX (UID 1): ArgumentError: cannot handle it\n").count(), 3, "{out}");
    assert!(out.contains("[imap] 127.0.0.1/INBOX (UID 1): given up (its handler failed 3 times): flagged\n"), "{out}");
    let mut connection = grenat_db::connect(&format!("sqlite://{}", path.display())).unwrap();
    let events = grenat_ops::events::latest(connection.as_mut(), false, 100).unwrap();
    let email: Vec<_> = events.iter().filter(|e| e.source == "email").collect();
    assert_eq!(email.len(), 4, "{events:?}");
    assert!(email.iter().all(|e| e.subject == "127.0.0.1/INBOX (UID 1)"), "{events:?}");
    assert_eq!(email.iter().filter(|e| e.error == "ArgumentError").count(), 3);
    assert_eq!(email.iter().filter(|e| e.error == "EmailError").count(), 1);
}

#[test]
fn served_a_dropped_connection_is_opened_again() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.deliver("INBOX", &message("one"));
    server.deliver("INBOX", &message("two"));
    // the connection breaks while the first read is under way
    server.drop_at(5);
    let url = server.url("imaps", "INBOX");
    let output = serve(
        &format!("on_email \"{url}\", every: 0.1 do |email|\n  puts \"handled #{{email.subject.trust!}}\"\nend\n"),
        &server,
        None,
    );
    wait_until("both seen", &output, || seen(&server, "INBOX") == 2);
    std::thread::sleep(Duration::from_millis(300));
    let out = output.lock().unwrap().clone();
    assert_eq!(out.matches("handled one\n").count(), 1, "{out}");
    assert_eq!(out.matches("handled two\n").count(), 1, "{out}");
    assert!(server.connections() >= 2);
}

#[test]
fn served_a_secret_url_never_shows_its_password() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.deliver("INBOX", &message("hello"));
    let root = temp_dir("email-credentials");
    let location = grenat_config::credentials::Location::of(&root, None);
    location.create().unwrap();
    let good = server.url("imaps", "INBOX");
    let bad = good.replace(PASSWORD, "hunter2-wrong");
    location.write(&format!("support:\n  imap_url: \"{good}\"\n  wrong_url: \"{bad}\"\n")).unwrap();
    let src = "on_email Credentials.fetch(:support, :imap_url), every: 0.1 do |email|\n  puts \"handled #{email.subject.trust!}\"\nend\non_email Credentials.fetch(:support, :wrong_url), every: 0.1 do |email|\n  puts \"never\"\nend\n";
    let output = serve(src, &server, Some(root));
    wait_until("seen", &output, || seen(&server, "INBOX") == 1);
    wait_until("the wrong one refused", &output, || {
        output.lock().unwrap().contains("[imap] 127.0.0.1/INBOX: ImapError")
    });
    std::thread::sleep(Duration::from_millis(300));
    let out = output.lock().unwrap().clone();
    assert!(out.contains("handled hello\n"), "{out}");
    assert!(!out.contains(PASSWORD) && !out.contains("hunter2"), "{out}");
}

#[test]
fn outside_tests_a_url_must_name_a_mailbox() {
    let parsed = grenat_parser::parse("on_email \"test-support-imap_url\" do |e|\nend\n");
    let e = run_main(&parsed.program, Vec::new(), Options::default()).unwrap_err();
    assert_eq!(e.ty, "ArgumentError");
}
