//! The `setter` binary, against local git repositories.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-setter-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn setter(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_setter")).args(args).current_dir(dir).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", text(&out.stderr));
}

fn commit(dir: &Path, message: &str) {
    if !dir.join(".git").exists() {
        git(dir, &["init", "--quiet", "--initial-branch", "main"]);
    }
    git(dir, &["add", "."]);
    git(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", message]);
}

#[test]
fn a_facet_is_created_published_listed_and_added() {
    let dir = temp_dir("cycle");
    // a facet: created, committed, published
    let out = setter(&dir, &["new", "greet"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let facet = dir.join("greet");
    for file in ["grenat.toml", "Facetfile", "src/lib.grn", "tests/lib_test.grn", "README.md"] {
        assert!(facet.join(file).is_file(), "no {file}");
    }
    let out = setter(&facet, &["publish"]);
    assert!(text(&out.stderr).contains("not a git repository"), "{}", text(&out.stderr));
    commit(&facet, "first");
    let out = Command::new(env!("CARGO_BIN_EXE_setter"))
        .arg("publish")
        .current_dir(&facet)
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stderr).starts_with("✓ tagged v0.1.0"), "{}", text(&out.stderr));
    let again = setter(&facet, &["publish"]);
    assert!(text(&again.stderr).contains("v0.1.0 is already tagged"), "{}", text(&again.stderr));

    // an index lists it
    let index = dir.join("index");
    std::fs::create_dir_all(index.join("facets")).unwrap();
    std::fs::write(index.join("facets/greet.toml"), format!("git = \"{}\"\n", facet.display())).unwrap();
    commit(&index, "index");

    // an application adds it
    let app = dir.join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("grenat.toml"), "[package]\nname = \"app\"\n").unwrap();
    assert!(setter(&app, &["init"]).status.success());
    let facetfile = app.join("Facetfile");
    let head = std::fs::read_to_string(&facetfile).unwrap();
    std::fs::write(&facetfile, format!("{head}source \"{}\"\n", index.display())).unwrap();
    let out = setter(&app, &["add", "greet"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("✓ added `facet \"greet\", \"~> 0.1\"`"), "{}", text(&out.stderr));
    assert!(std::fs::read_to_string(&facetfile).unwrap().ends_with("facet \"greet\", \"~> 0.1\"\n"));
    let list = setter(&app, &["list"]);
    assert!(text(&list.stderr).contains("greet 0.1.0 (index:"), "{}", text(&list.stderr));
    assert!(app.join(".grenat/facets/greet-0.1.0/src/lib.grn").is_file());
    let twice = setter(&app, &["add", "greet"]);
    assert!(text(&twice.stderr).contains("already in the Facetfile"));
    let missing = setter(&app, &["add", "nowhere"]);
    assert!(text(&missing.stderr).contains("in none of the indexes"), "{}", text(&missing.stderr));
    assert_eq!(setter(&app, &["frobnicate"]).status.code(), Some(2));
}

#[test]
fn a_trusted_native_facet_is_built_at_install() {
    let dir = temp_dir("native");
    let facet = grenat_native::fixture::copy_facet(&dir);
    let app = dir.join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("grenat.toml"), "[package]\nname = \"app\"\n").unwrap();
    let install = |facetfile: &str| {
        std::fs::write(app.join("Facetfile"), facetfile).unwrap();
        // the fixtures' target directory: its dependencies are built already
        Command::new(env!("CARGO_BIN_EXE_setter"))
            .arg("install")
            .current_dir(&app)
            .env("CARGO_TARGET_DIR", grenat_native::fixture::target_dir())
            .output()
            .unwrap()
    };
    let refused = install("facet \"sheets\", path: \"../sheets\"\n");
    assert!(!refused.status.success());
    assert!(text(&refused.stderr).contains("trust it with `facet \"sheets\", native: true` in the Facetfile"));
    let out = install("facet \"sheets\", path: \"../sheets\", native: true\n");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("sheets 0.1.0 (path:../sheets, native: 12 Rust function(s))"),
        "{}",
        text(&out.stderr)
    );
    let declarations = std::fs::read_to_string(facet.join(".grenat/native/native.grn")).unwrap();
    assert!(declarations.contains("native def read_sheet(path: String) -> ~Array(Array(String)) uses fs.read\n"));
    assert!(facet.join(".grenat/native/manifest.json").is_file());
    let list = setter(&app, &["list"]);
    assert!(text(&list.stderr).contains("native: 12 Rust function(s)"), "{}", text(&list.stderr));
}

#[test]
fn a_trusted_bridge_facet_describes_itself_at_install() {
    use grenat_bridge::fixture::{self, RUBY};
    if let Some(why) = fixture::missing(RUBY) {
        println!("skipped: {why}");
        return;
    }
    let dir = temp_dir("bridge");
    let facet = fixture::copy_facet(RUBY, &dir);
    let app = dir.join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("grenat.toml"), "[package]\nname = \"app\"\n").unwrap();
    let install = |facetfile: &str| {
        std::fs::write(app.join("Facetfile"), facetfile).unwrap();
        setter(&app, &["install"])
    };
    let refused = install("facet \"texts\", path: \"../texts\"\n");
    assert!(!refused.status.success());
    assert!(text(&refused.stderr).contains("trust it with `facet \"texts\", bridge: true` in the Facetfile"));
    let out = install("facet \"texts\", path: \"../texts\", bridge: true\n");
    assert!(out.status.success(), "{}", text(&out.stderr));
    let functions = fixture::installed(RUBY).unwrap().1.manifest.functions.len();
    let line =
        format!("texts 0.1.0 (path:../texts, bridge: {functions} function(s) served by `ruby bridge/server.rb`)");
    assert!(text(&out.stderr).contains(&line), "{}", text(&out.stderr));
    let declarations = std::fs::read_to_string(facet.join(".grenat/bridge/bridge.grn")).unwrap();
    assert!(declarations.contains("native def shout(text: String) -> ~String\n"));
    let list = setter(&app, &["list"]);
    assert!(text(&list.stderr).contains(&line), "{}", text(&list.stderr));
}
