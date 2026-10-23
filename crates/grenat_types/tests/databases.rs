//! Databases on the network (E0300): a literal PostgreSQL URL whose host
//! is not this machine is `net` on that host, for `Db.connect` and
//! `database` alike; SQLite, local servers and configured URLs need none.

mod common;

use common::*;

fn open(uses: &str, url: &str) -> String {
    format!("def main uses {uses}\n  db = Db.connect({url})\n  p db.query(\"SELECT 1\")\nend\n")
}

#[test]
fn a_remote_server_is_net_on_its_host() {
    let url = "\"postgres://app:s3cr3t@db.acme.io:5432/app\"";
    let src = open("db.read", url);
    let d = single(&src, "E0300", &format!("Db.connect({url})"));
    assert_eq!(d.help.as_deref(), Some("add `net(\"db.acme.io\")` to `uses`"));
    assert!(!d.message.contains("s3cr3t"), "{}", d.message);
    for uses in ["db.read, net(\"db.acme.io\")", "db.read, net"] {
        clean(&open(uses, url));
    }
    single(&open("db.read, net(\"api.acme.io\")", url), "E0300", &format!("Db.connect({url})"));
    // every host it names
    let both = "\"postgres://u@a.acme.io,b.acme.io/app?sslmode=require\"";
    let d = single(&open("db.read, net(\"a.acme.io\")", both), "E0300", &format!("Db.connect({both})"));
    assert_eq!(d.help.as_deref(), Some("add `net(\"b.acme.io\")` to `uses`"));
    clean(&open("db.read, net(\"a.acme.io\"), net(\"b.acme.io\")", both));
    let query = "\"postgres:///app?host=db.acme.io\"";
    single(&open("db.read", query), "E0300", &format!("Db.connect({query})"));
}

#[test]
fn local_databases_need_no_net() {
    for url in [
        "sqlite://app.db",
        "sqlite::memory:",
        "postgres://mehdifarsi@localhost:15432/postgres",
        "postgres://u:p@127.0.0.1/app",
        "postgresql://u@[::1]:5432/app",
        "postgres:///app",
        "postgres://u@%2Fvar%2Frun%2Fpostgresql/app",
        "postgres:///app?host=/var/run/postgresql",
    ] {
        clean(&open("db.read", &format!("\"{url}\"")));
    }
}

#[test]
fn a_configured_url_is_checked_at_run_time() {
    clean(&open("db.read, env", "Env.fetch(\"DATABASE_URL\")"));
    clean(&open("db.read, env", "Credentials.fetch(:db, :url)"));
}

#[test]
fn the_effect_falls_on_the_code_around() {
    // the top level declares no `uses`: `database` is declared there
    clean("database \"postgres://app:pw@db.acme.io/app\"\ndb = Db.connect(\"postgres://db.acme.io/app\")\n");
    let src = "def setup uses db.write\n  database \"postgres://db.acme.io/app\"\nend\n";
    let d = single(src, "E0300", "database \"postgres://db.acme.io/app\"");
    assert_eq!(d.help.as_deref(), Some("add `net(\"db.acme.io\")` to `uses`"));
    clean("def setup uses net(\"db.acme.io\")\n  database \"postgres://db.acme.io/app\"\nend\n");
    // a helper passes it on to its callers
    let src = "def open = Db.connect(\"postgres://db.acme.io/app\")\ndef main uses db.read\n  open\nend\n";
    single(src, "E0300", "open");
    // connecting gives the same handle every run: no `step` in a workflow
    clean(
        "workflow w(n: Int) uses db.read, net(\"db.acme.io\")\n  db = Db.connect(\"postgres://db.acme.io/app\")\n  n\nend\n",
    );
}
