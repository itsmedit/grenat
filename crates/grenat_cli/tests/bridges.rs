//! Bridge facets through `grenat`: an application trusting the Ruby facet
//! `texts` runs, checks and logs its functions (a declared variable passed
//! on to the server); the Python facet `numbers` tests its own; `grenat
//! build` refuses them for now.
//!
//! A test whose interpreter is not installed says so and returns.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use grenat_bridge::fixture::{self, PYTHON, RUBY};

fn grenat_in(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("ANTHROPIC_API_KEY")
        .envs(env.iter().copied())
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-cli-bridges-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

#[test]
fn an_application_runs_and_checks_its_bridge_functions() {
    if let Some(why) = fixture::missing(RUBY) {
        println!("skipped: {why}");
        return;
    }
    let dir = temp_dir("run");
    fixture::copy_facet(RUBY, &dir);
    let app = dir.join("app");
    write(&app.join("grenat.toml"), "[package]\nname = \"app\"\n");
    write(&app.join("Facetfile"), "facet \"texts\", path: \"../texts\", bridge: true\n");
    write(
        &app.join("src/main.grn"),
        "require \"texts\"\n\ndef main\n  puts loud_words(\"a b\").join(\",\"), add(40, 2), shout(\"hi\").trust!\n  p greeting.trust!\nend\n",
    );
    grenat_package::facets::install(&app, false).unwrap();

    let out = grenat_in(&app, &["run"], &[("TEXTS_GREETING", "hello")]);
    assert_eq!(text(&out.stdout), "A,B\n42\nHI\n\"hello\"\n", "{}", text(&out.stderr));
    // a variable the facet does not declare is not passed on
    let out = grenat_in(&app, &["run"], &[]);
    assert_eq!(text(&out.stdout), "A,B\n42\nHI\nnil\n", "{}", text(&out.stderr));
    let out = grenat_in(&app, &["check"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));

    // `--log`: the calls, and what the server writes on standard error
    write(&app.join("src/chatty.grn"), "require \"texts\"\n\ndef main\n  puts chatter(\"x\").trust!\nend\n");
    let out = grenat_in(&app, &["run", "src/chatty.grn"], &[("GRENAT_LOG", "1")]);
    let err = text(&out.stderr);
    assert!(err.contains("[bridge] texts: chatter\n") && err.contains("[bridge] texts: warned x\n"), "{err}");

    // an effect not declared: a checker error, before running
    write(&app.join("src/bad.grn"), "require \"texts\"\n\ndef main uses net\n  read_text(\"x\")\nend\n");
    let out = grenat_in(&app, &["check", "src/bad.grn"], &[]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("error[E0300]: `main` uses effect `fs.read` without declaring it"), "{err}");

    // `grenat build` cannot put a bridge into an executable yet
    let out = grenat_in(&app, &["build"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("calls native code (`native def "), "{}", text(&out.stderr));

    // trust withdrawn: refused, and why
    write(&app.join("Facetfile"), "facet \"texts\", path: \"../texts\"\n");
    let out = grenat_in(&app, &["run"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("trust it with `facet \"texts\", bridge: true` in the Facetfile"));
}

#[test]
fn a_bridge_facet_tests_its_own_functions() {
    if let Some(why) = fixture::missing(PYTHON) {
        println!("skipped: {why}");
        return;
    }
    let facet = fixture::copy_facet(PYTHON, &temp_dir("own"));
    grenat_package::facets::install(&facet, false).unwrap();
    let out = grenat_in(&facet, &["test"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stderr).ends_with("1 passed, 0 failed\n"), "{}", text(&out.stderr));
}
