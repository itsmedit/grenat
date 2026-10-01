//! The same behavior on SQLite and on PostgreSQL (when `GRENAT_TEST_POSTGRES`
//! is a database URL: only temporary tables are used).

use grenat_db::{Cell, Connection, connect, vectors};

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
    assert_eq!(
        db.execute(insert, &[Cell::Int(1), Cell::Text("Ada".into()), Cell::Float(9.5), Cell::Bool(true)]).unwrap(),
        1
    );
    db.execute(insert, &[Cell::Int(2), Cell::Text("O'Brien; DROP TABLE orders".into()), Cell::Null, Cell::Bool(false)])
        .unwrap();

    let rows = db.query("SELECT id, customer, total FROM orders WHERE id >= ? ORDER BY id", &[Cell::Int(1)]).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        vec![
            ("id".into(), Cell::Int(1)),
            ("customer".into(), Cell::Text("Ada".into())),
            ("total".into(), Cell::Float(9.5))
        ]
    );
    // a value is never SQL: the "injection" is stored as it is
    assert_eq!(rows[1][1].1, Cell::Text("O'Brien; DROP TABLE orders".into()));
    assert_eq!(rows[1][2].1, Cell::Null);
    // a `?` in a literal is not a placeholder
    let rows = db
        .query("SELECT '?' AS mark, count(*) AS n FROM orders WHERE customer = ?", &[Cell::Text("Ada".into())])
        .unwrap();
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

/// Vectors stored as bytes, and the column type a migration gets.
fn vectors_as_bytes(db: &mut dyn Connection, temp: &str) {
    let (column, note) = vectors::column_type(db, 3).unwrap();
    assert_eq!(note, None);
    db.batch(&format!("CREATE {temp} TABLE docs (id INTEGER PRIMARY KEY, embedding {column})")).unwrap();
    assert_eq!(vectors::storage(db, "docs", "embedding").unwrap(), vectors::Storage::Bytes);
    let v = [0.5, -1.0, 0.25];
    db.execute("INSERT INTO docs (id, embedding) VALUES (?, ?)", &[Cell::Int(1), Cell::Blob(vectors::encode(&v))])
        .unwrap();
    db.execute("INSERT INTO docs (id, embedding) VALUES (?, ?)", &[Cell::Int(2), Cell::Null]).unwrap();
    let rows = db.query("SELECT embedding FROM docs ORDER BY id", &[]).unwrap();
    assert_eq!(vectors::from_cell(&rows[0][0].1).unwrap(), Some(v.to_vec()));
    assert_eq!(rows[1][0].1, Cell::Null);
}

#[test]
fn sqlite_vectors() {
    let mut db = connect("sqlite::memory:").unwrap();
    assert_eq!(vectors::column_type(db.as_mut(), 1536).unwrap(), ("BLOB".to_string(), None));
    vectors_as_bytes(db.as_mut(), "");
}

#[test]
fn postgres_vectors() {
    let Ok(url) = std::env::var("GRENAT_TEST_POSTGRES") else {
        eprintln!("GRENAT_TEST_POSTGRES not set: vectors on PostgreSQL not tested");
        return;
    };
    let mut db = connect(&url).unwrap();
    let available =
        !db.query("SELECT 1 AS found FROM pg_available_extensions WHERE name = 'vector'", &[]).unwrap().is_empty();
    if !available {
        // the server has no pgvector: vectors are bytes, compared in Rust
        vectors_as_bytes(db.as_mut(), "TEMP");
        assert!(vectors::storage(db.as_mut(), "nope", "embedding").unwrap_err().contains("no column"));
        db.batch("CREATE TEMP TABLE texts (body TEXT)").unwrap();
        assert!(vectors::storage(db.as_mut(), "texts", "body").unwrap_err().contains("is a `text`"));
        eprintln!("the PostgreSQL server has no pgvector: pgvector's own storage not tested");
        return;
    }
    // in a transaction, as a migration: pgvector is enabled if the role may
    db.batch("BEGIN").unwrap();
    let (column, note) = vectors::column_type(db.as_mut(), 3).unwrap();
    if let Some(note) = note {
        eprintln!("pgvector not enabled ({note}): pgvector's own storage not tested");
        db.batch("ROLLBACK").unwrap();
        return;
    }
    assert_eq!(column, "vector(3)");
    db.batch("CREATE TEMP TABLE docs (id INTEGER PRIMARY KEY, embedding vector(3))").unwrap();
    assert_eq!(vectors::storage(db.as_mut(), "docs", "embedding").unwrap(), vectors::Storage::PgVector);
    db.execute(
        "INSERT INTO docs (id, embedding) VALUES (?, ?::text::vector), (?, ?::text::vector)",
        &[Cell::Int(1), Cell::Text("[1,0,0]".into()), Cell::Int(2), Cell::Text("[0,1,0]".into())],
    )
    .unwrap();
    let rows = db
        .query(
            "SELECT id, embedding::text AS embedding FROM docs ORDER BY embedding <=> ?::text::vector",
            &[Cell::Text("[0.1,1,0]".into())],
        )
        .unwrap();
    assert_eq!(rows[0][0].1, Cell::Int(2));
    assert_eq!(vectors::from_cell(&rows[0][1].1).unwrap(), Some(vec![0.0, 1.0, 0.0]));
    db.batch("ROLLBACK").unwrap();
}
