//! Packages: manifests, requires, path and git dependencies, the lock file.

use std::path::{Path, PathBuf};
use std::process::Command;

use grenat_package::{Bundle, LoadError, Lock, Manifest, Reference, Source, create, load};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-package-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The files of a bundle, by name, in load order.
fn files(bundle: &Bundle) -> Vec<String> {
    bundle
        .sources
        .files
        .iter()
        .map(|f| Path::new(&f.path).file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn error(result: Result<Bundle, LoadError>) -> String {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(LoadError::Message(m)) => m,
        Err(LoadError::Diagnostics { sources, diagnostics }) => {
            format!("{}: {}", sources.files[0].path, diagnostics[0].message)
        }
    }
}

fn manifest(name: &str, deps: &str) -> String {
    format!("[package]\nname = \"{name}\"\n\n[dependencies]\n{deps}")
}

#[test]
fn manifests_are_parsed_and_checked() {
    let m = Manifest::parse(
        "[package]\nname = \"app\"\nversion = \"1.2.0\"\n[dependencies]\nutils = { path = \"../utils\" }\nhttp = { git = \"https://x/http\", tag = \"v1\" }\nlatest = { git = \"https://x/l\" }\n",
    )
    .unwrap();
    assert_eq!((m.name.as_str(), m.version.as_str()), ("app", "1.2.0"));
    assert_eq!(m.main, Path::new("src/main.grn"));
    assert_eq!(m.lib, Path::new("src/lib.grn"));
    assert_eq!(m.dependency("utils").unwrap().source, Source::Path("../utils".into()));
    assert_eq!(
        m.dependency("http").unwrap().source,
        Source::Git { url: "https://x/http".into(), reference: Reference::Tag("v1".into()) }
    );
    assert_eq!(m.dependency("latest").unwrap().source, Source::Git { url: "https://x/l".into(), reference: Reference::Default });

    let invalid = |text: &str| Manifest::parse(text).unwrap_err();
    assert_eq!(invalid("x = 1"), "missing the `[package]` table");
    assert_eq!(invalid("[package]\nversion = \"1\""), "missing `package.name`");
    assert!(invalid("[package]\nname = \"My-App\"").starts_with("invalid package name `My-App`"));
    let dep = |spec: &str| invalid(&format!("[package]\nname = \"a\"\n[dependencies]\nd = {spec}"));
    assert_eq!(dep("\"1.0\""), "dependency `d`: expected `{ path = … }` or `{ git = … }`");
    assert_eq!(dep("{ path = \"x\", git = \"y\" }"), "dependency `d`: give either `path` or `git`");
    assert_eq!(dep("{ git = \"y\", tag = \"a\", rev = \"b\" }"), "dependency `d`: give one of `branch`, `tag` or `rev`");
    assert_eq!(dep("{ path = \"x\", version = \"1\" }"), "dependency `d`: unknown key `version`");
    assert_eq!(invalid("[package\nname = 1"), "unclosed table, expected `]`");
}

#[test]
fn relative_requires_load_each_file_once_dependencies_first() {
    let dir = temp_dir("relative");
    write(&dir.join("main.grn"), "require \"./lib/a\"\nrequire \"./lib/b.grn\"\nputs a + b\n");
    write(&dir.join("lib/a.grn"), "require \"./shared\"\ndef a = shared\n");
    write(&dir.join("lib/b.grn"), "require \"./shared\"\nrequire \"../main\"\ndef b = shared\n");
    write(&dir.join("lib/shared.grn"), "def shared = 1");
    let bundle = load(&dir.join("main.grn"), false).unwrap();
    assert_eq!(files(&bundle), ["shared.grn", "a.grn", "b.grn", "main.grn"]);
    assert!(bundle.package.is_none());
    // spans of the whole program point into the right file
    let text = &bundle.sources.text;
    let at = text.find("def b").unwrap() as u32;
    let (file, span) = bundle.sources.locate(grenat_ast::Span { start: at, end: at + 5 });
    assert!(file.path.ends_with("lib/b.grn"), "{}", file.path);
    assert_eq!(span.start, "require \"./shared\"\nrequire \"../main\"\n".len() as u32);
    // the program parses as a whole
    let parsed = grenat_parser::parse(text);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
}

#[test]
fn require_errors_point_at_the_require() {
    let dir = temp_dir("errors");
    write(&dir.join("main.grn"), "require \"./missing\"\n");
    let e = error(load(&dir.join("main.grn"), false));
    assert!(e.contains("main.grn: cannot find `./missing` (no file "), "{e}");
    assert!(e.ends_with("missing.grn)"), "{e}");

    write(&dir.join("dyn.grn"), "name = \"x\"\nrequire \"./#{name}\"\n");
    assert!(error(load(&dir.join("dyn.grn"), false)).ends_with("`require` takes a literal path, without interpolation"));
    write(&dir.join("pkg.grn"), "require \"http\"\n");
    let e = error(load(&dir.join("pkg.grn"), false));
    assert!(e.ends_with("`require \"http\"` needs a dependency named `http` in a grenat.toml, or a path: `./http`"), "{e}");
    write(&dir.join("broken.grn"), "require \"./syntax\"\n");
    write(&dir.join("syntax.grn"), "def f(\n");
    let e = error(load(&dir.join("broken.grn"), false));
    assert!(e.starts_with("syntax.grn: ") || e.contains("/syntax.grn: "), "{e}");
}

#[test]
fn path_dependencies_and_package_files() {
    let dir = temp_dir("path");
    write(&dir.join("app/grenat.toml"), &manifest("app", "utils = { path = \"../utils\" }\n"));
    write(&dir.join("app/src/main.grn"), "require \"utils\"\nrequire \"utils/text\"\nrequire \"app/helpers\"\n");
    write(&dir.join("app/src/helpers.grn"), "def helper = 1\n");
    write(&dir.join("utils/grenat.toml"), &manifest("utils", ""));
    write(&dir.join("utils/src/lib.grn"), "def util = 1\n");
    write(&dir.join("utils/src/text.grn"), "def text = 1\n");
    let bundle = load(&dir.join("app/src/main.grn"), false).unwrap();
    assert_eq!(files(&bundle), ["lib.grn", "text.grn", "helpers.grn", "main.grn"]);
    assert_eq!(bundle.package.unwrap().manifest.name, "app");
    // no git dependency: no lock file
    assert!(!dir.join("app/grenat.lock").exists());

    write(&dir.join("app/src/main.grn"), "require \"nope\"\n");
    let e = error(load(&dir.join("app/src/main.grn"), false));
    assert!(e.contains("unknown package `nope`: add `facet \"nope\"` to the Facetfile"), "{e}");
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A git repository holding the package `greet`; returns its first commit.
fn greet_repo(dir: &Path) -> String {
    write(&dir.join("grenat.toml"), &manifest("greet", ""));
    write(&dir.join("src/lib.grn"), "def greet = \"v1\"\n");
    git(dir, &["init", "--quiet", "--initial-branch", "main"]);
    git(dir, &["add", "."]);
    git(dir, &["commit", "--quiet", "-m", "v1"]);
    git(dir, &["tag", "v1"]);
    git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn git_dependencies_are_fetched_and_locked() {
    let dir = temp_dir("git");
    let repo = dir.join("greet");
    let v1 = greet_repo(&repo);
    let url = repo.to_string_lossy().into_owned();
    write(&dir.join("app/grenat.toml"), &manifest("app", &format!("greet = {{ git = \"{url}\" }}\n")));
    write(&dir.join("app/src/main.grn"), "require \"greet\"\n");
    let main = dir.join("app/src/main.grn");

    let bundle = load(&main, false).unwrap();
    assert!(bundle.sources.text.contains("\"v1\""));
    assert!(dir.join("app/.grenat/deps/greet/src/lib.grn").is_file());
    let lock = Lock::load(&dir.join("app")).unwrap();
    assert_eq!(lock.entries["greet"].commit, v1);
    assert_eq!(lock.entries["greet"].reference, "default");

    // a program of the package that does not need `greet` keeps its pin
    write(&dir.join("app/tests/other.grn"), "def unrelated = 1\n");
    load(&dir.join("app/tests/other.grn"), false).unwrap();
    assert_eq!(Lock::load(&dir.join("app")).unwrap().entries["greet"].commit, v1);

    // a new commit upstream: the lock keeps the old one…
    write(&repo.join("src/lib.grn"), "def greet = \"v2\"\n");
    git(&repo, &["commit", "--quiet", "-am", "v2"]);
    let v2 = git(&repo, &["rev-parse", "HEAD"]);
    std::fs::remove_dir_all(dir.join("app/.grenat")).unwrap();
    assert!(load(&main, false).unwrap().sources.text.contains("\"v1\""));
    // …until an update
    assert!(load(&main, true).unwrap().sources.text.contains("\"v2\""));
    assert_eq!(Lock::load(&dir.join("app")).unwrap().entries["greet"].commit, v2);

    // a tag
    write(&dir.join("app/grenat.toml"), &manifest("app", &format!("greet = {{ git = \"{url}\", tag = \"v1\" }}\n")));
    assert!(load(&main, false).unwrap().sources.text.contains("\"v1\""));
    let lock = Lock::load(&dir.join("app")).unwrap();
    assert_eq!((lock.entries["greet"].reference.as_str(), lock.entries["greet"].commit.as_str()), ("tag=v1", v1.as_str()));

    // a repository that does not exist
    write(&dir.join("app/grenat.toml"), &manifest("app", "greet = { git = \"/nowhere/at/all\" }\n"));
    let e = error(load(&main, false));
    assert!(e.contains("`git clone --quiet /nowhere/at/all"), "{e}");
}

#[test]
fn the_lock_file_round_trips() {
    let text = "[[package]]\nname = \"a\"\ngit = \"https://x/a\"\nreference = \"tag=v1\"\ncommit = \"abc\"\n";
    let lock = Lock::parse(text).unwrap();
    assert_eq!(Lock::parse(&lock.render()).unwrap(), lock);
    assert!(Lock::parse("[[package]]\nname = \"a\"\n").is_err());
}

#[test]
fn a_new_package_loads() {
    let dir = temp_dir("new");
    create(&dir.join("hello"), "hello").unwrap();
    let bundle = load(&dir.join("hello/src/main.grn"), false).unwrap();
    assert_eq!(files(&bundle), ["lib.grn", "main.grn"]);
    assert_eq!(bundle.package.unwrap().manifest.name, "hello");
    assert!(load(&dir.join("hello/tests/lib_test.grn"), false).is_ok());
    assert_eq!(create(&dir.join("hello"), "hello").unwrap_err(), format!("{} already exists", dir.join("hello").display()));
    assert!(create(&dir.join("x"), "Bad").unwrap_err().starts_with("invalid package name"));
}

#[test]
fn an_overlay_replaces_files_on_disk() {
    let dir = temp_dir("overlay");
    write(&dir.join("main.grn"), "require \"./lib\"\n");
    write(&dir.join("lib.grn"), "def on_disk = 1\n");
    let mut overlay = std::collections::HashMap::new();
    overlay.insert(dir.join("lib.grn"), "require \"./unsaved\"\ndef edited = 1\n".to_string());
    overlay.insert(dir.join("unsaved.grn"), "def fresh = 1\n".to_string());
    let bundle = grenat_package::load_with(&dir.join("main.grn"), false, &overlay).unwrap();
    assert!(bundle.sources.text.contains("def edited") && bundle.sources.text.contains("def fresh"));
    assert!(!bundle.sources.text.contains("on_disk"));
}
