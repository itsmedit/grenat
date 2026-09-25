//! The same behavior on SQLite and on PostgreSQL (when `GRENAT_TEST_POSTGRES`
//! is a database URL: only temporary tables are used).

use grenat_db::{Cell, Connection, connect};

fn exercise(db: &mut dyn Connection, temp: &str) {
    let key = db.dialect().primary_key();
    db.batch(&format!("CREATE {temp} TABLE counters (id {key}, n INTEGER)")).unwrap();
    db.execute("INSERT INTO counters (n) VALUES (?)", &[Cell::Int(1)]).unwrap();
    let rows = db.query("INSERT INTO counters (n) VALUES (?) RETURNING id", &[Cell::Int(2)]).unwrap();
    assert_eq!(rows[0][0].1, Cell::Int(2), "ids are given by the database");
    db.batch(&format!(
        "CREATE {temp} TABLE orders (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, total FLOAT, paid BOOLEAN)"
    ))
    .unwrap();
    let insert = "INSERT INTO orders (id, customer, total, paid) VALUES (?, ?, ?, ?)";
    assert_eq!(db.execute(insert, &[Cell::Int(1), Cell::Text("Ada".into()), Cell::Float(9.5), Cell::Bool(true)]).unwrap(), 1);
    db.execute(insert, &[Cell::Int(2), Cell::Text("O'Brien; DROP TABLE orders".into()), Cell::Null, Cell::Bool(false)]).unwrap();

    let rows = db.query("SELECT id, customer, total FROM orders WHERE id >= ? ORDER BY id", &[Cell::Int(1)]).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], vec![("id".into(), Cell::Int(1)), ("customer".into(), Cell::Text("Ada".into())), ("total".into(), Cell::Float(9.5))]);
    // a value is never SQL: the "injection" is stored as it is
    assert_eq!(rows[1][1].1, Cell::Text("O'Brien; DROP TABLE orders".into()));
    assert_eq!(rows[1][2].1, Cell::Null);
    // a `?` in a literal is not a placeholder
    let rows = db.query("SELECT '?' AS mark, count(*) AS n FROM orders WHERE customer = ?", &[Cell::Text("Ada".into())]).unwrap();
    assert_eq!(rows[0][0].1, Cell::Text("?".into()));
    assert_eq!(rows[0][1].1, Cell::Int(1));
    assert!(db.query("SELECT nope FROM orders", &[]).unwrap_err().contains("nope"));
}

#[test]
fn sqlite() {
    let mut db = connect("sqlite::memory:").unwrap();
    exercise(db.as_mut(), "");
    let file = std::env::temp_dir().join(format!("grenat-db-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let mut db = connect(&format!("sqlite://{}", file.display())).unwrap();
    db.batch("CREATE TABLE t (x INTEGER)").unwrap();
    drop(db);
    assert!(file.exists());
    std::fs::remove_file(file).unwrap();
}

#[test]
fn postgres() {
    let Ok(url) = std::env::var("GRENAT_TEST_POSTGRES") else {
        eprintln!("GRENAT_TEST_POSTGRES not set: PostgreSQL not tested");
        return;
    };
    let mut db = connect(&url).unwrap();
    exercise(db.as_mut(), "TEMP");
    // unsupported column types say how to cast them
    let e = db.query("SELECT now() AS at", &[]).unwrap_err();
    assert!(e.contains("cast it in the query"), "{e}");
    assert_eq!(db.query("SELECT now()::text AS at", &[]).unwrap().len(), 1);
}

#[test]
fn urls() {
    assert!(connect("mysql://x").err().unwrap().starts_with("unsupported database URL"));
    assert!(connect("postgres://nobody@127.0.0.1:1/x").err().unwrap().starts_with("cannot connect"));
}
