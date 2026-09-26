//! `Db`: queries, typed records, transactions, capabilities, taint.

mod common;

use common::*;

const SCHEMA: &str = "\
struct Order
  id: Int
  customer: String
  total: Float
end
db = Db.connect(\"URL\")
db.migrate(\"CREATE TABLE orders (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, total FLOAT NOT NULL)\")
db.execute(\"INSERT INTO orders (id, customer, total) VALUES (?, ?, ?)\", [1, \"Ada\", 9.5])
db.execute(\"INSERT INTO orders (id, customer, total) VALUES (?, ?, ?)\", [2, \"Linus\", 20.0])
";

fn sqlite(src: &str) -> String {
    SCHEMA.replace("URL", "sqlite::memory:") + src
}

#[test]
fn queries_give_hashes_or_records() {
    let out = run(&sqlite(
        "\
p db.query(\"SELECT customer, total FROM orders WHERE total > ? ORDER BY id\", [5])
orders = db.query(\"SELECT * FROM orders ORDER BY total DESC\", as: Order)
p orders.first
p orders.map { |o| o.total }.sum
p db.first(\"SELECT * FROM orders WHERE id = ?\", [2], as: Order).customer
p db.first(\"SELECT * FROM orders WHERE id = ?\", [99], as: Order)
p db.execute(\"UPDATE orders SET total = total * 2\")
",
    ));
    assert_eq!(
        out,
        "[{\"customer\" => \"Ada\", \"total\" => 9.5}, {\"customer\" => \"Linus\", \"total\" => 20.0}]\n\
         Order(id: 2, customer: \"Linus\", total: 20.0)\n29.5\n\"Linus\"\nnil\n2\n"
    );
}

#[test]
fn a_transaction_commits_or_rolls_back() {
    let out = run(&sqlite(
        "\
db.transaction do
  db.execute(\"DELETE FROM orders WHERE id = ?\", [1])
end
begin
  db.transaction do
    db.execute(\"DELETE FROM orders\")
    raise \"changed my mind\"
  end
rescue RuntimeError => e
  puts e.message
end
p db.query(\"SELECT id FROM orders\")
",
    ));
    assert_eq!(out, "changed my mind\n[{\"id\" => 2}]\n");
}

#[test]
fn sql_errors_and_bad_parameters() {
    let e = run_err(&sqlite("db.query(\"SELECT nope FROM orders\")\n"), Vec::new());
    assert_eq!(e.ty, "DbError");
    assert!(e.message.contains("nope"), "{}", e.message);
    let e = run_err(&sqlite("db.query(\"SELECT 1\", [[1]])\n"), Vec::new());
    assert_eq!(e.message, "a database parameter cannot be [1]");
    let e = run_err("Db.connect(\"mysql://x\")\n", Vec::new());
    assert!(e.message.starts_with("unsupported database URL"), "{}", e.message);
}

#[test]
fn reads_and_writes_are_capabilities() {
    let src = sqlite(
        "\
def count(db: Database) -> Int uses db.read
  db.query(\"SELECT id FROM orders\").size
end
def purge(db: Database) -> Int uses db.read
  db.execute(\"DELETE FROM orders\")
end
def admin(db: Database) -> Int uses db
  db.execute(\"DELETE FROM orders WHERE id = 99\")
end
p count(db), admin(db)
purge(db)
",
    );
    let r = run_with(&src, Vec::new(), &[]);
    assert_eq!(r.output, "2\n0\n");
    let e = r.err();
    assert_eq!(e.ty, "CapabilityError");
    assert_eq!(e.message, "`db.write` is not allowed by `purge` (uses db.read)");
}

#[test]
fn sql_is_never_untrusted_and_untrusted_values_are_never_written() {
    let head = format!("{SUMMARY}{}s = summarize(\"x\")\n", sqlite(""));
    let e = run_err(&format!("{head}db.query(s.title)\n"), vec![summary_reply()]);
    assert_eq!(
        (e.ty.as_str(), e.message.as_str()),
        ("TaintError", "an untrusted value is SQL text in `query`: pass values as parameters")
    );
    // an untrusted value may filter a read…
    let out = run_with(
        &format!("{head}p db.query(\"SELECT id FROM orders WHERE customer = ?\", [s.title])\n"),
        vec![summary_reply()],
        &[],
    );
    assert_eq!(out.ok(), "[]\n");
    // …never be written, unless checked
    let e =
        run_err(&format!("{head}db.execute(\"UPDATE orders SET customer = ?\", [s.title])\n"), vec![summary_reply()]);
    assert_eq!(e.ty, "TaintError");
    let checked = format!(
        "{head}t = s.title.check {{ |t| t.size < 50 }}?\np db.execute(\"UPDATE orders SET customer = ?\", [t])\n"
    );
    assert_eq!(run_with(&checked, vec![summary_reply()], &[]).ok(), "2\n");
}

#[test]
fn postgres() {
    let Ok(url) = std::env::var("GRENAT_TEST_POSTGRES") else {
        eprintln!("GRENAT_TEST_POSTGRES not set: PostgreSQL not tested");
        return;
    };
    let src = SCHEMA.replace("URL", &url).replace("CREATE TABLE", "CREATE TEMP TABLE")
        + "p db.query(\"SELECT * FROM orders WHERE total > ? ORDER BY id\", [5], as: Order).map { |o| o.customer }\n";
    assert_eq!(run(&src), "[\"Ada\", \"Linus\"]\n");
}
