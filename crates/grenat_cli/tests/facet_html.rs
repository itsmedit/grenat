//! The official facet `html` (`facets/html`), a native facet: its crate
//! built once per test binary (as `setter install` builds it, into a shared
//! target directory) and shipped as the copy's prebuilt library; then
//! installed into an application that trusts it, whose programs and tests
//! `grenat` checks and runs with every function of the facet, error cases
//! and taint included — a hostile selector or page too. The facet's own
//! Grenat tests and Rust unit tests run here too, and its README is held to
//! what its crate can do.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use grenat_native::fixture::target_dir;

const FACET: &str = "html";

/// The facet in this checkout.
fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../facets").join(FACET).canonicalize().expect("facets/html")
}

/// The facet's library, built once (release, as `setter install` does).
fn library() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        grenat_native::build::build(&source().join("native"), Some(&target_dir()))
            .unwrap_or_else(|e| panic!("the facet `html` cannot be built: {e}"))
    })
}

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

/// A copy of the facet's sources, without what a build or an install left.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == ".grenat" || name == "target" || name == "prebuilt" {
            continue;
        }
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to.join(&name));
        } else {
            std::fs::copy(entry.path(), to.join(&name)).unwrap();
        }
    }
}

/// A directory with a copy of the facet (`html/`, its library prebuilt for
/// this platform) and an application (`app/`) whose Facetfile is
/// `facetfile`, its facets installed when the Facetfile trusts them.
fn world(name: &str, facetfile: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("grenat-cli-facet-html-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let facet = dir.join(FACET);
    copy_dir(&source(), &facet);
    let prebuilt = grenat_native::layout::prebuilt(FACET, &facet.join("native"));
    std::fs::create_dir_all(prebuilt.parent().unwrap()).unwrap();
    std::fs::copy(library(), &prebuilt).unwrap();
    let app = dir.join("app");
    write(&app.join("grenat.toml"), "[package]\nname = \"app\"\n");
    write(&app.join("Facetfile"), facetfile);
    (app, facet)
}

const TRUSTED: &str = "facet \"html\", path: \"../html\", native: true\n";

/// An application trusting the facet, installed.
fn app(name: &str) -> PathBuf {
    let (app, _) = world(name, TRUSTED);
    grenat_package::facets::install(&app, false).unwrap();
    app
}

const MENU: &str = r##"require "html"

PAGE = <<~HTML
  <h1>The <em>caf&eacute;</em></h1>
  <ul><li class="drink">Tea</li><li class="drink">Coffee</li><li>Cake</li></ul>
  <a href="/about">About</a> <a href="mailto:hi@example.com">Mail</a> <a href="drinks?page=2">More</a>
  <img src="/logo.png" alt="Logo"><img alt="none">
  <table id="prices"><tr><th>Item</th><th>Price</th></tr><tr><td>Tea</td><td>3</td></tr><tr><td>Cake</td></tr></table>
HTML

BASE = "https://cafe.example.com/menu/"

def main
  puts select(PAGE, "li.drink").join("|")
  puts select_attr(PAGE, "img", "src").join("|")
  puts select_html(PAGE, "h1 em").join("|")
  puts links(PAGE, BASE).join("|")
  puts all_links(PAGE, BASE).join("|")
  puts table(PAGE, "#prices").map { |row| row.join(",") }.join(";")
  puts table_records(PAGE, "#prices").map { |r| "#{r["Item"]}=#{r["Price"]}" }.join(";")
  puts select_first(PAGE, "h1"), matches?(PAGE, "nav")
  begin
    select(PAGE, "li[")
  rescue HtmlError => e
    puts e.message
  end
  begin
    links(PAGE, "menu/")
  rescue HtmlError => e
    puts e.message
  end
  begin
    table(PAGE, "ul")
  rescue HtmlError => e
    puts e.message
  end
end
"##;

#[test]
fn an_application_runs_and_checks_every_function() {
    let app = app("run");
    write(&app.join("src/main.grn"), MENU);
    let out = grenat_in(&app, &["run"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "Tea|Coffee\n\
         /logo.png\n\
         <em>café</em>\n\
         https://cafe.example.com/about|https://cafe.example.com/menu/drinks?page=2\n\
         https://cafe.example.com/about|mailto:hi@example.com|https://cafe.example.com/menu/drinks?page=2\n\
         Item,Price;Tea,3;Cake\n\
         Tea=3;Cake=\n\
         The café\nfalse\n\
         invalid CSS selector `li[`: it ends too early\n\
         invalid base URL `menu/`: relative URL without a base\n\
         `ul` matches a <ul>, not a <table>\n"
    );
    let out = grenat_in(&app, &["check"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn an_applications_tests_use_the_facet() {
    let app = app("tests");
    write(
        &app.join("src/lib.grn"),
        "require \"html\"\n\n## The titles of a page's articles.\ndef titles(page: String) -> Array(String)\n  select(page, \"article h2\")\nend\n",
    );
    write(
        &app.join("tests/titles_test.grn"),
        r##"require "../src/lib"

test "titles of a page fetched" do
  mock_http "GET https://news.example.com/*", body: "<article><h2>One</h2></article><article><h2> Two </h2></article>"
  page = Http.get("https://news.example.com/").body
  assert_equal ["One", "Two"], titles(page)
  assert titles(page).tainted?
  assert !titles("<article><h2>Mine</h2></article>").tainted?
end

test "errors are HtmlErrors" do
  e = assert_raises(HtmlError) { select("<p>", "p >") }
  assert_equal "invalid CSS selector `p >`: dangling combinator", e.message
  assert_raises(HtmlError) { select_attr("<p>", "[", "href") }
  assert_raises(HtmlError) { select_html("<p>", "") }
  assert_raises(HtmlError) { all_links("<a href=x>", "nowhere") }
  assert_raises(HtmlError) { table("<p id=x>", "#x") }
end
"##,
    );
    let out = grenat_in(&app, &["test"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(err.ends_with("2 passed, 0 failed\n"), "{err}");
}

#[test]
fn what_an_untrusted_page_says_stays_untrusted() {
    let app = app("taint");
    // a link found in a fetched page cannot be fetched unchecked
    let crawl = "require \"html\"\n\ndef crawl(url: String) -> Int uses net\n  page = Http.get(url).body\n  next_url = links(page, url).first\n  Http.get(next_url).status\nend\n";
    write(&app.join("src/crawl.grn"), crawl);
    let out = grenat_in(&app, &["check", "src/crawl.grn"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(
        err.contains("error[E0412]: an untrusted value reaches `Http.get` (effect `net`) without validation"),
        "{err}"
    );
    assert!(err.contains("Http.get(next_url)"), "{err}");

    // checked, it may go
    let checked = crawl.replace(
        "links(page, url).first\n",
        "links(page, url).first.check { |u| u.start_with?(\"https://shop.example.com/\") }?\n",
    );
    write(&app.join("src/crawl.grn"), &checked);
    write(
        &app.join("tests/crawl_test.grn"),
        r##"require "../src/crawl"

test "a checked link is followed, another is refused" do
  mock_http "GET https://shop.example.com/", body: "<a href=\"/next\">next</a>"
  mock_http "GET https://shop.example.com/next", status: 204
  assert_equal 204, crawl("https://shop.example.com/")
  mock_http "GET https://evil.example.com/", body: "<a href=\"https://elsewhere.example.com/\">x</a>"
  assert_raises(CheckError) { crawl("https://evil.example.com/") }
end
"##,
    );
    let out = grenat_in(&app, &["check", "src/crawl.grn"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let out = grenat_in(&app, &["test"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(err.ends_with("1 passed, 0 failed\n"), "{err}");
}

#[test]
fn the_checker_knows_the_functions() {
    let app = app("types");
    write(
        &app.join("src/bad.grn"),
        "require \"html\"\n\ndef main\n  select(1, \"p\")\n  table(\"<table>\")\n  puts select_attr(\"<p>\", \"p\", \"id\").size + \"x\"\nend\n",
    );
    let out = grenat_in(&app, &["check", "src/bad.grn"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.matches("error[E0200]").count() >= 3, "{err}");
    // pure functions: no effect to declare, nor in the declarations setter wrote
    let declarations = std::fs::read_to_string(app.join("../html/.grenat/native/native.grn")).unwrap();
    let natives: Vec<&str> = declarations.lines().filter(|l| l.starts_with("native def ")).collect();
    assert_eq!(natives.len(), 6, "{declarations}");
    assert!(
        natives.iter().all(|l| l.ends_with(" pure") && !l.contains(" uses ") && !l.contains('~')),
        "{declarations}"
    );
    assert!(
        declarations.contains(
            "native def select_attr(html: String, selector: String, attribute: String) -> Array(String) pure"
        )
    );
    assert!(declarations.contains("native def table(html: String, selector: String) -> Array(Array(String)) pure"));
}

const HOSTILE: &str = r##"require "html"

def main
  n = 200000
  begin
    puts select("<p>x</p>", ":not(" * n + "p" + ")" * n).size
  rescue HtmlError => e
    puts e.message
  end
  begin
    puts select_html("<p>x</p>", ":not(" * 600 + "p" + ")" * 600).size
  rescue HtmlError => e
    puts e.message
  end
  deep = "<div>" * 100000
  begin
    puts select(deep, "div " * 100000 + "p").size
  rescue HtmlError => e
    puts e.message
  end
  begin
    puts select(deep, "p").size
  rescue HtmlError => e
    puts e.message
  end
  begin
    puts links(deep, "https://e.com/").size
  rescue HtmlError => e
    puts e.message
  end
  puts matches?("<p>x</p>", ":not(" * 32 + "p" + ")" * 32)
end
"##;

/// A selector or a page an attacker wrote: an `HtmlError` the program
/// rescues, soon — not a stack overflow killing the process (it did, with a
/// SIGBUS), nor minutes of native code no timeout stops.
#[test]
fn a_hostile_selector_or_page_is_an_error_the_program_rescues() {
    let app = app("hostile");
    write(&app.join("src/main.grn"), HOSTILE);
    let started = Instant::now();
    let out = grenat_in(&app, &["run"]);
    let elapsed = started.elapsed();
    assert_eq!(out.status.code(), Some(0), "{:?}: {}", out.status, text(&out.stderr));
    let too_deep = "the HTML nests too deeply: more than 512 elements inside one another";
    assert_eq!(
        text(&out.stdout),
        format!(
            "invalid CSS selector `{nots}…`: it is too long (more than 4096 bytes)\n\
             invalid CSS selector `{nots}…`: it nests too deeply (more than 32 parentheses inside one another)\n\
             invalid CSS selector `{divs}…`: it is too long (more than 4096 bytes)\n\
             {too_deep}\n{too_deep}\ntrue\n",
            nots = ":not(".repeat(8),
            divs = "div ".repeat(10),
        )
    );
    assert!(elapsed < Duration::from_secs(60), "{elapsed:?}");
}

/// Installing by version needs a crate that builds outside this checkout:
/// while it depends on `grenat_ext` by a path into it, the README installs
/// the facet by path only.
#[test]
fn the_readme_installs_the_facet_only_as_its_crate_builds() {
    let manifest = std::fs::read_to_string(source().join("native/Cargo.toml")).unwrap();
    let readme = std::fs::read_to_string(source().join("README.md")).unwrap();
    let sdk_by_path = manifest.lines().any(|l| l.starts_with("grenat_ext ") && l.contains("path ="));
    let installs: Vec<&str> = readme.lines().filter(|l| l.trim_start().starts_with("facet \"html\"")).collect();
    assert!(!installs.is_empty(), "{readme}");
    if sdk_by_path {
        for line in installs {
            assert!(line.contains(" path: ") && line.contains("native: true"), "{line}");
        }
    }
}

#[test]
fn the_facet_must_be_trusted_with_native_code() {
    let (app, _) = world("untrusted", "facet \"html\", path: \"../html\"\n");
    let e = grenat_package::facets::install(&app, false).unwrap_err();
    assert!(e.contains("facet `html` ships native code (Rust)"), "{e}");
    assert!(e.contains("`facet \"html\", path: \"../html\", native: true`"), "{e}");
}

#[test]
fn the_facet_passes_its_own_tests() {
    let (_, facet) = world("own", TRUSTED);
    grenat_package::facets::install(&facet, false).unwrap();
    let out = grenat_in(&facet, &["test"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(err.ends_with(" passed, 0 failed\n"), "{err}");
    assert!(!err.contains("✗"), "{err}");
    let mut files = vec!["check".to_string(), "src/lib.grn".to_string()];
    for entry in std::fs::read_dir(facet.join("tests")).unwrap() {
        files.push(format!("tests/{}", entry.unwrap().file_name().to_string_lossy()));
    }
    assert!(files.len() >= 5, "{files:?}");
    let out = grenat_in(&facet, &files.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn the_native_crate_passes_its_own_tests() {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args(["test", "--quiet", "--manifest-path"])
        .arg(source().join("native/Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir())
        .output()
        .unwrap();
    let report = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{report}");
    assert!(report.contains("test result: ok."), "{report}");
}
