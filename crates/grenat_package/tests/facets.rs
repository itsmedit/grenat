//! Facets: an index, versions from tags, transitive facets, the lock file.

use std::path::{Path, PathBuf};
use std::process::Command;

use grenat_package::facets::{install, installed};
use grenat_package::load;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-facets-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
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
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Commits `files` in the repository `dir` (created if needed) and tags it.
fn release(dir: &Path, files: &[(&str, &str)], tag: &str) {
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "--quiet", "--initial-branch", "main"]);
    }
    for (path, text) in files {
        write(&dir.join(path), text);
    }
    git(dir, &["add", "."]);
    git(dir, &["commit", "--quiet", "-m", tag]);
    git(dir, &["tag", "-a", tag, "-m", tag]);
}

fn manifest(name: &str, version: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n")
}

/// An index listing `greet` and `shout`, and their repositories.
fn world(dir: &Path) -> String {
    let greet = dir.join("greet");
    for version in ["1.0.0", "1.2.0", "2.0.0"] {
        release(
            &greet,
            &[("grenat.toml", &manifest("greet", version)), ("src/lib.grn", &format!("def greet = \"greet {version}\"\n"))],
            &format!("v{version}"),
        );
    }
    let index = dir.join("index");
    let shout = dir.join("shout");
    release(
        &shout,
        &[
            ("grenat.toml", &manifest("shout", "0.1.0")),
            ("Facetfile", &format!("source \"{}\"\nfacet \"greet\", \"~> 1.0\"\n", index.display())),
            ("src/lib.grn", "require \"greet\"\n\ndef shout = greet.upcase\n"),
        ],
        "v0.1.0",
    );
    let entry = |repo: &Path| format!("git = \"{}\"\n", repo.display());
    release(&index, &[("facets/greet.toml", &entry(&greet)), ("facets/shout.toml", &entry(&shout))], "index");
    index.to_string_lossy().into_owned()
}

fn app(dir: &Path, facetfile: &str) -> PathBuf {
    let app = dir.join("app");
    write(&app.join("grenat.toml"), &manifest("app", "0.1.0"));
    write(&app.join("Facetfile"), facetfile);
    write(&app.join("src/main.grn"), "require \"shout\"\nrequire \"greet\"\n\ndef main\n  puts shout, greet\nend\n");
    app
}

fn versions(root: &Path) -> Vec<String> {
    installed(root).unwrap().iter().map(|f| format!("{} {}", f.name, f.version.unwrap())).collect()
}

#[test]
fn facets_are_resolved_fetched_locked_and_required() {
    let dir = temp_dir("resolve");
    let index = world(&dir);
    let app = app(&dir, &format!("source \"{index}\"\nfacet \"shout\"\n"));
    // shout asks for greet ~> 1.0: the highest 1.x
    install(&app, false).unwrap();
    assert_eq!(versions(&app), ["greet 1.2.0", "shout 0.1.0"]);
    let bundle = load(&app.join("src/main.grn"), false).unwrap();
    assert!(bundle.sources.text.contains("\"greet 1.2.0\""), "{}", bundle.sources.text);
    assert!(bundle.sources.text.contains("def shout = greet.upcase"));
    let lock = std::fs::read_to_string(app.join("Facetfile.lock")).unwrap();
    assert!(lock.contains("dir = \".grenat/facets/greet-1.2.0\""), "{lock}");

    // a new release: the lock keeps the old one, until an update
    release(&dir.join("greet"), &[("src/lib.grn", "def greet = \"greet 1.3.0\"\n"), ("grenat.toml", &manifest("greet", "1.3.0"))], "v1.3.0");
    install(&app, false).unwrap();
    assert_eq!(versions(&app), ["greet 1.2.0", "shout 0.1.0"]);
    install(&app, true).unwrap();
    assert_eq!(versions(&app), ["greet 1.3.0", "shout 0.1.0"]);
}

#[test]
fn requirements_must_agree() {
    let dir = temp_dir("conflict");
    let index = world(&dir);
    let app = app(&dir, &format!("source \"{index}\"\nfacet \"greet\", \"~> 2.0\"\nfacet \"shout\"\n"));
    let e = install(&app, false).unwrap_err();
    assert!(e.starts_with("no version of `greet` satisfies"), "{e}");
    let app = app_named(&dir, "lost", &format!("source \"{index}\"\nfacet \"nowhere\"\n"));
    let e = install(&app, false).unwrap_err();
    assert!(e.starts_with("facet `nowhere` is in none of the indexes"), "{e}");
}

fn app_named(dir: &Path, name: &str, facetfile: &str) -> PathBuf {
    let app = dir.join(name);
    write(&app.join("grenat.toml"), &manifest(name, "0.1.0"));
    write(&app.join("Facetfile"), facetfile);
    app
}

#[test]
fn a_facet_must_be_installed_before_it_is_required() {
    let dir = temp_dir("missing");
    let index = world(&dir);
    let app = app(&dir, &format!("source \"{index}\"\nfacet \"shout\"\n"));
    let e = match load(&app.join("src/main.grn"), false) {
        Err(grenat_package::LoadError::Diagnostics { diagnostics, .. }) => diagnostics[0].message.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(e, "facet `shout` is not installed: run `setter install`");
}

#[test]
fn path_facets_are_locked_relative_to_the_package() {
    let dir = temp_dir("path");
    write(&dir.join("utils/grenat.toml"), &manifest("utils", "0.2.0"));
    write(&dir.join("utils/src/lib.grn"), "def util = 1\n");
    let app = app_named(&dir, "app", "facet \"utils\", path: \"../utils\"\n");
    write(&app.join("src/main.grn"), "require \"utils\"\n");
    install(&app, false).unwrap();
    let facets = installed(&app).unwrap();
    assert_eq!(facets[0].dir, Path::new("../utils"));
    assert_eq!(facets[0].version.unwrap().to_string(), "0.2.0");
    assert!(load(&app.join("src/main.grn"), false).unwrap().sources.text.contains("def util"));
    // a facet must be the package it claims to be
    let wrong = app_named(&dir, "wrong", "facet \"tools\", path: \"../utils\"\n");
    assert!(install(&wrong, false).unwrap_err().ends_with("is named `utils`, not `tools`"));
}
