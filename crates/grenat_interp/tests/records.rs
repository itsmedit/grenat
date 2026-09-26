//! Records: structs stored in the application's database, migrations.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, migrate, run_tests};

const APP: &str = "\
struct Ticket
  table :tickets
  id: Int?
  subject: String
  status: String = \"open\"
  urgent: Bool = false
end

migration \"001_create_tickets\" do |db|
  db.migrate(\"CREATE TABLE tickets (id INTEGER PRIMARY KEY, subject TEXT NOT NULL, status TEXT NOT NULL, urgent BOOLEAN NOT NULL)\")
end
";

fn results(src: &str) -> Vec<(String, Option<String>)> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    run_tests(&parsed.program, options)
        .unwrap()
        .into_iter()
        .map(|o| (o.name, o.error.map(|e| format!("{}: {}", e.ty, e.message))))
        .collect()
}

fn all_pass(src: &str) {
    for (name, error) in results(src) {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
}

#[test]
fn create_find_where_save_delete() {
    all_pass(&format!(
        "{APP}
test \"a record's life\" do
  t = Ticket.create(subject: \"Bug\", urgent: true)
  assert_equal 1, t.id
  assert_equal \"open\", Ticket.find(1).status
  assert Ticket.find(1).urgent
  Ticket.create(subject: \"Question\")
  assert_equal 2, Ticket.count
  assert_equal [\"Bug\"], Ticket.where(urgent: true).map {{ |x| x.subject }}
  t.with(status: \"closed\").save
  assert_equal \"closed\", Ticket.find(1).status
  assert_equal 1, Ticket.count(status: \"open\")
  assert_equal [1, 2], Ticket.all.map {{ |x| x.id }}
  t.delete
  assert_equal nil, Ticket.find(1)
  assert_equal 1, Ticket.count
end
test \"each test has a database of its own\" do
  assert_equal 0, Ticket.count
end
"
    ));
}

#[test]
fn untrusted_values_are_never_stored_unchecked() {
    let results = results(&format!(
        "{APP}
post \"/tickets\" do |req|
  Ticket.create(subject: req.json[\"subject\"])
end
get \"/tickets\" do |req|
  json(Ticket.where(status: req.params[\"status\"]))
end
test \"refused\" do
  request :post, \"/tickets\", json: {{subject: \"x\"}}
end
test \"a read may filter on it\" do
  assert_equal \"[]\", (request :get, \"/tickets?status=open\")[\"body\"]
end
"
    ));
    assert!(
        results[0].1.as_deref().unwrap().starts_with("TaintError: an untrusted value reaches a `Ticket` written"),
        "{results:?}"
    );
    assert_eq!(results[1].1, None);
}

#[test]
fn migrations_are_applied_once() {
    let dir = temp_dir("migrate");
    let file = dir.join("app.db");
    let src = format!(
        "database \"sqlite://{}\"\nmigration \"001_a\" do |db|\n  db.migrate(\"CREATE TABLE a (x INTEGER)\")\nend\nmigration \"002_b\" do |db|\n  db.migrate(\"CREATE TABLE b (y INTEGER)\")\nend\n",
        file.display()
    );
    let parsed = grenat_parser::parse(&src);
    let options = || Options { journal: Some(temp_dir("journal")), ..Options::default() };
    assert_eq!(migrate(&parsed.program, options()).unwrap(), ["001_a", "002_b"]);
    assert!(migrate(&parsed.program, options()).unwrap().is_empty());
    // a failing migration leaves nothing behind
    let bad = format!(
        "{src}migration \"003_bad\" do |db|\n  db.migrate(\"CREATE TABLE c (z INTEGER)\")\n  raise \"oops\"\nend\n"
    );
    let parsed = grenat_parser::parse(&bad);
    assert_eq!(migrate(&parsed.program, options()).unwrap_err().message, "oops");
    let parsed = grenat_parser::parse(&format!("{bad}\nnope\n").replace("  raise \"oops\"\n", ""));
    assert_eq!(migrate(&parsed.program, options()).unwrap_err().ty, "NameError");
    let e = migrate(&grenat_parser::parse("migration \"x\" do |db|\nend\n").program, options()).unwrap_err();
    assert!(e.message.starts_with("no database"), "{}", e.message);
}

#[test]
fn a_record_s_class_methods_call_where_and_create_without_a_receiver() {
    all_pass(
        "struct Note
  table :notes
  id: Int?
  key: String
  text: String

  def self.remember(key: String, text: String) -> Note uses db
    known = where(key:).first
    known ? known.with(text:).save : create(key:, text:)
  end
end
migration \"001_notes\" do |db|
  db.migrate(\"CREATE TABLE notes (id #{db.primary_key}, key TEXT NOT NULL, text TEXT NOT NULL)\")
end
test \"remembered once\" do
  Note.remember(\"a\", \"v1\")
  Note.remember(\"a\", \"v2\")
  assert_equal 1, Note.count
  assert_equal \"v2\", Note.find(1)&.text
end
",
    );
}
