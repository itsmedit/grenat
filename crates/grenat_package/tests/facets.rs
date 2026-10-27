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
            &[
                ("grenat.toml", &manifest("greet", version)),
                ("src/lib.grn", &format!("def greet = \"greet {version}\"\n")),
            ],
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
    release(
        &dir.join("greet"),
        &[("src/lib.grn", "def greet = \"greet 1.3.0\"\n"), ("grenat.toml", &manifest("greet", "1.3.0"))],
        "v1.3.0",
    );
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

/// A repository holding the facet `html` in `facets/html`, tagged
/// `html-v…`, and its own `v…` tags (not the facet's); and an index whose
/// entries name that directory and prefix. Returns the index.
fn monorepo(dir: &Path) -> String {
    let repo = dir.join("tools");
    let html = |version: &str| {
        [
            ("facets/html/grenat.toml".to_string(), manifest("html", version)),
            ("facets/html/src/lib.grn".to_string(), format!("def html = \"html {version}\"\n")),
        ]
    };
    for (version, tag) in [("0.1.0", "html-v0.1.0"), ("0.1.1", "html-v0.1.1")] {
        let files = html(version);
        let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        release(&repo, &files, tag);
    }
    // 0.2.0 has facets of its own, in its own Facetfile: next to it in the repository
    release(
        &repo,
        &[
            ("facets/html/grenat.toml", &manifest("html", "0.2.0")),
            ("facets/html/Facetfile", "facet \"css\", path: \"../css\"\n"),
            ("facets/html/src/lib.grn", "require \"css\"\n\ndef html = \"html 0.2.0 \" + css\n"),
            ("facets/css/grenat.toml", &manifest("css", "0.1.0")),
            ("facets/css/src/lib.grn", "def css = \"css\"\n"),
        ],
        "html-v0.2.0",
    );
    // the repository's own release: not a version of the facet
    release(&repo, &[("grenat.toml", &manifest("tools", "0.9.0"))], "v0.9.0");
    let index = dir.join("index");
    let entry = |path: &str, prefix: &str| {
        format!("git = \"{}\"\npath = \"{path}\"\ntag_prefix = \"{prefix}\"\n", repo.display())
    };
    release(
        &index,
        &[
            ("facets/html.toml", &entry("facets/html", "html-v")),
            ("facets/gone.toml", &entry("facets/gone", "html-v")),
            ("facets/untagged.toml", &entry("facets/html", "untagged-v")),
            ("facets/outside.toml", &entry("../elsewhere", "html-v")),
            ("facets/absolute.toml", &entry("/etc", "html-v")),
            ("facets/spaced.toml", &entry("facets/html", "html v")),
        ],
        "index",
    );
    index.to_string_lossy().into_owned()
}

#[test]
fn a_facet_in_a_directory_of_a_repository_has_its_own_tags() {
    let dir = temp_dir("monorepo");
    let index = monorepo(&dir);
    let app = app_named(&dir, "app", &format!("source \"{index}\"\nfacet \"html\"\n"));
    write(&app.join("src/main.grn"), "require \"html\"\n\ndef main\n  puts html\nend\n");
    // the highest html-v… tag, not v0.9.0
    assert_eq!(grenat_package::facets::latest(&app, "html").unwrap().to_string(), "0.2.0");
    install(&app, false).unwrap();
    // its own facet, from its Facetfile in that directory
    assert_eq!(versions(&app), ["css 0.1.0", "html 0.2.0"]);
    let facets = installed(&app).unwrap();
    assert_eq!(facets[0].dir, Path::new(".grenat/facets/html-0.2.0/facets/css"));
    assert_eq!(facets[1].dir, Path::new(".grenat/facets/html-0.2.0/facets/html"));
    let tag = Command::new("git").args(["rev-parse", "html-v0.2.0^{commit}"]).current_dir(dir.join("tools")).output();
    let tag = String::from_utf8(tag.unwrap().stdout).unwrap();
    assert_eq!(facets[1].commit.as_deref(), Some(tag.trim()));
    let lock = std::fs::read_to_string(app.join("Facetfile.lock")).unwrap();
    assert!(lock.contains("dir = \".grenat/facets/html-0.2.0/facets/html\""), "{lock}");
    let bundle = load(&app.join("src/main.grn"), false).unwrap();
    assert!(bundle.sources.text.contains("\"html 0.2.0 \" + css"), "{}", bundle.sources.text);
    assert!(bundle.sources.text.contains("def css = \"css\""), "{}", bundle.sources.text);
}

#[test]
fn a_requirement_picks_among_the_prefixed_versions() {
    let dir = temp_dir("monorepo-requirement");
    let index = monorepo(&dir);
    let app = app_named(&dir, "app", &format!("source \"{index}\"\nfacet \"html\", \"~> 0.1.0\"\n"));
    install(&app, false).unwrap();
    assert_eq!(versions(&app), ["html 0.1.1"]);
    // v0.9.0 is the repository's, not the facet's
    let app = app_named(&dir, "nine", &format!("source \"{index}\"\nfacet \"html\", \"= 0.9.0\"\n"));
    assert_eq!(install(&app, false).unwrap_err(), "no version of `html` satisfies = 0.9.0");
}

#[test]
fn a_bad_index_entry_is_named_in_the_error() {
    let dir = temp_dir("monorepo-errors");
    let index = monorepo(&dir);
    let error = |name: &str| {
        let app = app_named(&dir, name, &format!("source \"{index}\"\nfacet \"{name}\"\n"));
        install(&app, false).unwrap_err()
    };
    let entry = |name: &str| format!("the index entry facets/{name}.toml of {index}");
    assert_eq!(
        error("outside"),
        format!("{}: `path` must stay inside the repository: no `..` (`../elsewhere`)", entry("outside"))
    );
    assert_eq!(
        error("absolute"),
        format!("{}: `path` must be relative to the repository's root (`/etc`)", entry("absolute"))
    );
    assert_eq!(
        error("spaced"),
        format!("{}: `tag_prefix` must be non-empty and without spaces (`html v`)", entry("spaced"))
    );
    let e = error("gone");
    assert!(e.starts_with("facet `gone`: html-v0.2.0 of "), "{e}");
    assert!(e.ends_with(&format!("as {} says, has no directory `facets/gone`", entry("gone"))), "{e}");
    let e = error("untagged");
    assert!(e.starts_with("facet `untagged` has no version: no tag of "), "{e}");
    assert!(e.ends_with(&format!("such as untagged-v1.0.0 ({})", entry("untagged"))), "{e}");
}

#[test]
fn a_git_facet_may_name_its_directory_in_the_repository() {
    let dir = temp_dir("monorepo-git");
    monorepo(&dir);
    let repo = dir.join("tools");
    let facetfile =
        format!("facet \"html\", git: \"{}\", tag: \"html-v0.1.0\", path: \"facets/html\"\n", repo.display());
    let app = app_named(&dir, "app", &facetfile);
    write(&app.join("src/main.grn"), "require \"html\"\n");
    install(&app, false).unwrap();
    let facets = installed(&app).unwrap();
    assert_eq!(facets[0].dir, Path::new(".grenat/facets/html-tag_html-v0.1.0/facets/html"));
    assert_eq!(facets[0].version.unwrap().to_string(), "0.1.0");
    assert!(load(&app.join("src/main.grn"), false).unwrap().sources.text.contains("\"html 0.1.0\""));
    let facetfile = format!("facet \"html\", git: \"{}\", tag: \"html-v0.1.0\", path: \"nope\"\n", repo.display());
    let app = app_named(&dir, "nope", &facetfile);
    let e = install(&app, false).unwrap_err();
    assert!(e.starts_with("facet `html`: tag=html-v0.1.0 of ") && e.ends_with("has no directory `nope`"), "{e}");
}
