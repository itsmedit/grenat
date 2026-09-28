//! Native facets through `grenat`: an application trusting the facet
//! `sheets` runs, checks and tests its native functions; `grenat build`
//! refuses them for now.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use grenat_native::fixture::{self, FACET};

fn grenat_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("ANTHROPIC_API_KEY")
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

/// An application and the facet `sheets` next to it (its library prebuilt,
/// from the fixture), installed and trusted.
fn world(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("grenat-cli-natives-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let (_, installed) = fixture::installed();
    let facet = fixture::copy_facet(&dir);
    let prebuilt = grenat_native::layout::prebuilt(FACET, &facet.join("native"));
    std::fs::create_dir_all(prebuilt.parent().unwrap()).unwrap();
    std::fs::copy(&installed.library, &prebuilt).unwrap();
    let app = dir.join("app");
    write(&app.join("grenat.toml"), "[package]\nname = \"app\"\n");
    write(&app.join("Facetfile"), "facet \"sheets\", path: \"../sheets\", native: true\n");
    write(
        &app.join("src/main.grn"),
        "require \"sheets\"\n\ndef main uses fs.read\n  puts total([1, 2, 3]), add(40, 2), shout(\"hi\").trust!\n  rows = read_sheet(\"sheet.csv\").trust!\n  puts cells(rows).map { |c| c.text }.join(\"|\")\nend\n",
    );
    write(&app.join("sheet.csv"), "a, b\nc\n");
    grenat_package::facets::install(&app, false).unwrap();
    (app, facet)
}

#[test]
fn an_application_runs_and_checks_its_native_functions() {
    let (app, _) = world("run");
    let out = grenat_in(&app, &["run"]);
    assert_eq!(text(&out.stdout), "6\n42\nHI\na|b|c\n", "{}", text(&out.stderr));
    let out = grenat_in(&app, &["check"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));

    // a wrong argument type, an effect not declared: checker errors, before running
    write(
        &app.join("src/bad.grn"),
        "require \"sheets\"\n\ndef main uses net\n  puts add(1, \"two\")\n  read_sheet(\"x\")\nend\n",
    );
    let out = grenat_in(&app, &["check", "src/bad.grn"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("error[E0200]"), "{err}");
    assert!(err.contains("error[E0300]: `main` uses effect `fs.read` without declaring it"), "{err}");

    // `grenat build` cannot link native code yet
    let out = grenat_in(&app, &["build"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("calls native code (`native def "), "{err}");
    assert!(err.contains("`grenat build` cannot link native facets into an executable yet"), "{err}");

    // trust withdrawn: refused, and why
    write(&app.join("Facetfile"), "facet \"sheets\", path: \"../sheets\"\n");
    let out = grenat_in(&app, &["run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("trust it with `facet \"sheets\", native: true` in the Facetfile"));
}

#[test]
fn a_native_facet_tests_its_own_functions() {
    let (_, facet) = world("own");
    grenat_package::facets::install(&facet, false).unwrap();
    let out = grenat_in(&facet, &["test"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stderr).ends_with("1 passed, 0 failed\n"), "{}", text(&out.stderr));
}
