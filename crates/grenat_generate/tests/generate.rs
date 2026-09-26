//! Generators on disk: what they write, what they update, what they refuse.

use std::path::{Path, PathBuf};

use grenat_generate::{Change, Kind, create_app, generate};

/// A new application in a directory of its own.
fn app(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("grenat-generate-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let dir = base.join(name);
    create_app(&dir, name).unwrap();
    dir
}

fn read(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

const NOW: i64 = 1_790_000_000;

#[test]
fn a_new_application() {
    let dir = app("desk");
    assert!(read(&dir, "grenat.toml").contains("main = \"src/app.grn\""));
    assert!(read(&dir, "src/config.grn").starts_with("# The database"));
    let models = grenat_config::models::declarations(&read(&dir, "config/models.yml")).unwrap();
    assert!(models.starts_with("model :fast, provider: :anthropic"), "{models}");
    assert!(read(&dir, "src/app.grn").contains("require \"./config\""));
    assert!(read(&dir, "tests/app_test.grn").contains("request(:get, \"/health\")"));
    assert!(read(&dir, "README.md").starts_with("# desk\n"));
    assert!(dir.join("db").is_dir());
    let gitignore = read(&dir, ".gitignore");
    assert!(gitignore.contains("config/master.key") && gitignore.contains("config/credentials/*.key"), "{gitignore}");
    let credentials = grenat_config::credentials::Location::of(&dir, None);
    assert!(credentials.read().unwrap().contains("Credentials.fetch"), "created, and readable with its key");
    assert!(create_app(&dir, "desk").unwrap_err().contains("already exists"));
    assert!(create_app(&dir.with_file_name("Desk"), "Desk").unwrap_err().contains("invalid application name"));
}

#[test]
fn an_agent_and_its_test() {
    let dir = app("agents");
    let changes = generate(&dir, Kind::Agent, "support_desk", &[], NOW).unwrap();
    assert_eq!(
        changes,
        [
            Change::Created("src/agents/support_desk.grn".into()),
            Change::Created("tests/agents/support_desk_test.grn".into()),
            Change::Updated("src/app.grn".into()),
        ]
    );
    let agent = read(&dir, "src/agents/support_desk.grn");
    assert!(agent.contains("agent SupportDesk\n"));
    assert!(agent.contains("on SupportDeskRequest(text: String) -> ~String"));
    assert!(read(&dir, "tests/agents/support_desk_test.grn").contains("spawn SupportDesk"));
    assert!(read(&dir, "src/app.grn").contains("require \"./config\"\nrequire \"./agents/support_desk\"\n"));
}

#[test]
fn a_record_its_migration_and_its_test() {
    let dir = app("records");
    let fields = ["subject:String".to_string(), "score:Float?".to_string()];
    generate(&dir, Kind::Record, "category", &fields, NOW).unwrap();
    let record = read(&dir, "src/records/category.grn");
    assert!(
        record.contains("struct Category\n  table :categories\n  id: Int?\n  subject: String\n  score: Float?\nend")
    );
    let migration = read(&dir, "src/migrations/20260921141320_create_categories.grn");
    assert!(migration.contains("migration \"20260921141320_create_categories\" do |db|"));
    assert!(
        migration
            .contains("CREATE TABLE categories (id #{db.primary_key}, subject TEXT NOT NULL, score DOUBLE PRECISION)")
    );
    assert!(
        read(&dir, "tests/records/category_test.grn").contains("Category.create(subject: \"subject\", score: 1.5)")
    );
    let app = read(&dir, "src/app.grn");
    assert!(
        app.contains("require \"./records/category\"\nrequire \"./migrations/20260921141320_create_categories\"\n")
    );
}

#[test]
fn workflows_tools_and_evals() {
    let dir = app("parts");
    generate(&dir, Kind::Workflow, "onboard", &[], NOW).unwrap();
    generate(&dir, Kind::Tool, "lookup", &[], NOW).unwrap();
    assert!(read(&dir, "src/workflows/onboard.grn").contains("workflow onboard(id: Int) -> ~String uses llm, human"));
    assert!(read(&dir, "src/tools/lookup.grn").contains("tool lookup(query: String) -> String"));
    // an eval asks an agent
    assert!(generate(&dir, Kind::Eval, "triage", &[], NOW).unwrap_err().contains("no src/agents/triage.grn"));
    generate(&dir, Kind::Agent, "triage", &[], NOW).unwrap();
    let changes = generate(&dir, Kind::Eval, "triage", &[], NOW).unwrap();
    assert_eq!(changes.len(), 2, "an eval is not required by the application: {changes:?}");
    assert!(read(&dir, "evals/triage_eval.grn").contains("dataset: \"triage.jsonl\""));
    assert!(read(&dir, "evals/triage.jsonl").contains("\"expected\""));
}

#[test]
fn what_generators_refuse() {
    let dir = app("refusals");
    generate(&dir, Kind::Tool, "lookup", &[], NOW).unwrap();
    let before = read(&dir, "src/app.grn");
    // nothing is overwritten, nor half written
    std::fs::remove_file(dir.join("src/tools/lookup.grn")).unwrap();
    assert!(
        generate(&dir, Kind::Tool, "lookup", &[], NOW)
            .unwrap_err()
            .contains("tests/tools/lookup_test.grn already exists")
    );
    assert!(!dir.join("src/tools/lookup.grn").exists());
    assert_eq!(read(&dir, "src/app.grn"), before);
    let fields = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(generate(&dir, Kind::Record, "note", &[], NOW).unwrap_err().contains("a record has fields"));
    assert!(generate(&dir, Kind::Record, "note", &fields(&["text:Text"]), NOW).unwrap_err().contains("invalid field"));
    assert!(generate(&dir, Kind::Agent, "bot", &fields(&["x:Int"]), NOW).unwrap_err().contains("only a record"));
    assert!(generate(&dir, Kind::Agent, "Bot", &[], NOW).unwrap_err().contains("invalid name"));
    let package = dir.with_file_name("not_an_app");
    std::fs::create_dir_all(&package).unwrap();
    assert!(generate(&package, Kind::Agent, "bot", &[], NOW).unwrap_err().contains("is not an application"));
}
