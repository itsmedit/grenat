//! Bridge facets in an application: trusted explicitly, installed (their
//! server asked to describe itself), and loaded with the declarations
//! written from its answer.
//!
//! A test whose interpreter is not installed says so and returns.

use std::path::{Path, PathBuf};
use std::time::Duration;

use grenat_bridge::fixture::{self, PYTHON, RUBY};
use grenat_bridge::layout::Layout;
use grenat_package::facets::install;
use grenat_package::{LoadError, Manifest, bridge, load};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-bridge-facets-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// An application using the facet `dir/texts`, with this `Facetfile` line.
fn app(dir: &Path, line: &str) -> PathBuf {
    let app = dir.join("app");
    write(&app.join("grenat.toml"), "[package]\nname = \"app\"\n");
    write(&app.join("Facetfile"), &format!("{line}\n"));
    write(&app.join("src/main.grn"), "require \"texts\"\n\ndef main\n  puts loud_words(\"a b\"), add(1, 2)\nend\n");
    app
}

fn load_error(entry: &Path) -> String {
    match load(entry, false) {
        Err(LoadError::Diagnostics { diagnostics, .. }) => diagnostics[0].message.clone(),
        Err(LoadError::Message(message)) => message,
        Ok(_) => panic!("loaded"),
    }
}

#[test]
fn a_bridge_facet_is_refused_unless_trusted() {
    let dir = temp_dir("untrusted");
    let facet = fixture::copy_facet(RUBY, &dir);
    // trusted with native code is not trusted with a bridge
    let app = app(&dir, "facet \"texts\", path: \"../texts\", native: true");
    let e = install(&app, false).unwrap_err();
    assert_eq!(
        e,
        "facet `texts` ships a bridge (Ruby or Python code, run as a process), which runs outside Grenat's \
         sandbox: trust it with `facet \"texts\", bridge: true` in the Facetfile"
    );
    // nothing recorded, nothing started
    assert!(!app.join("Facetfile.lock").exists());
    assert!(!Layout::new(&facet).dir.exists());
}

#[test]
fn a_trusted_bridge_facet_is_installed_and_loaded_with_its_declarations() {
    if fixture::installed(RUBY).is_none() {
        return;
    }
    let dir = temp_dir("trusted");
    let facet = fixture::copy_facet(RUBY, &dir);
    let app = app(&dir, "facet \"texts\", path: \"../texts\", bridge: true");
    install(&app, false).unwrap();
    let layout = Layout::new(&facet);
    assert!(layout.manifest().is_file() && layout.declarations().is_file());
    assert!(layout.lib().join("ruby/grenat/bridge.rb").is_file());
    let bundle = load(&app.join("src/main.grn"), false).unwrap();
    assert!(bundle.sources.text.contains("native def add(a: Int, b: Int) -> Int pure\n"), "{}", bundle.sources.text);
    assert!(bundle.sources.text.contains("def loud_words(text: String) -> Array(String)"));
    let bridges = bridge::bridges(&app).unwrap();
    assert_eq!(bridges.len(), 1);
    assert_eq!((bridges[0].facet.as_str(), bridges[0].dir.canonicalize().unwrap()), (RUBY, facet.clone()));
    assert_eq!(bridges[0].spec, fixture::spec(RUBY));
    assert!(bridges[0].manifest.function("shout").is_some());
    let (count, command) = bridge::functions(RUBY, &facet).unwrap();
    assert_eq!((count, command.as_str()), (bridges[0].manifest.functions.len(), "ruby bridge/server.rb"));

    // a `native def` written by hand, in the application or anywhere else, is refused
    let forged = "require \"texts\"\n\nnative def read_text(path: String) -> String pure\n\nputs read_text(\"x\")\n";
    write(&app.join("src/forged.grn"), forged);
    assert_eq!(
        load_error(&app.join("src/forged.grn")),
        "`read_text` is a `native def` written by hand: `setter install` writes them"
    );
    write(&dir.join("alone.grn"), "native def shout(text: String) -> String pure\n");
    assert!(load_error(&dir.join("alone.grn")).starts_with("`shout` is a `native def` written by hand"));

    // trust withdrawn: the program is refused, and no bridge is given
    write(&app.join("Facetfile"), "facet \"texts\", path: \"../texts\"\n");
    assert!(load_error(&app.join("src/main.grn")).starts_with("facet `texts` ships a bridge"));
    assert!(bridge::bridges(&app).unwrap().is_empty());

    // trusted again, but its declarations gone: install it again
    write(&app.join("Facetfile"), "facet \"texts\", path: \"../texts\", bridge: true\n");
    std::fs::remove_file(layout.declarations()).unwrap();
    assert_eq!(
        load_error(&app.join("src/main.grn")),
        "the bridge of facet `texts` is not installed: run `setter install`"
    );
}

#[test]
fn a_bridge_facet_sees_its_own_functions() {
    if fixture::installed(PYTHON).is_none() {
        return;
    }
    let dir = temp_dir("own");
    let facet = fixture::copy_facet(PYTHON, &dir);
    let test = facet.join("tests/lib_test.grn");
    assert_eq!(load_error(&test), "the bridge of facet `numbers` is not installed: run `setter install`");
    // the facet's own code is its author's: trusted without a Facetfile line
    install(&facet, false).unwrap();
    let bundle = load(&test, false).unwrap();
    assert!(bundle.sources.text.contains("native def mean(values: Array(Float)) -> Float? pure\n"));
    assert_eq!(bridge::bridges(&facet).unwrap()[0].facet, PYTHON);
}

#[test]
fn a_server_that_cannot_describe_itself_fails_the_install() {
    let dir = temp_dir("broken");
    let facet = dir.join("broken");
    write(
        &facet.join("grenat.toml"),
        "[package]\nname = \"broken\"\n[bridge]\ncommand = [\"sh\", \"-c\", \"echo 'no server' >&2; exit 4\"]\n",
    );
    write(&facet.join("src/lib.grn"), "def x = 1\n");
    let app = dir.join("app");
    write(&app.join("grenat.toml"), "[package]\nname = \"app\"\n");
    write(&app.join("Facetfile"), "facet \"broken\", path: \"../broken\", bridge: true\n");
    let e = install(&app, false).unwrap_err();
    assert!(e.starts_with("facet `broken`: bridge: the process of facet `broken` exited with status 4"), "{e}");
    assert!(e.contains("no server"), "{e}");
}

#[test]
fn the_bridge_section_of_a_manifest() {
    let parse = |text: &str| Manifest::parse(&format!("[package]\nname = \"x\"\n{text}"));
    let spec = parse("[bridge]\ncommand = [\"ruby\", \"bridge/server.rb\"]\n").unwrap().bridge.unwrap();
    assert_eq!(spec, grenat_bridge::Spec::new(&["ruby", "bridge/server.rb"]));
    let spec = parse("[bridge]\ncommand = [\"python3\", \"s.py\"]\nenv = [\"API_URL\"]\ntimeout = 2.5\n")
        .unwrap()
        .bridge
        .unwrap();
    assert_eq!((spec.env, spec.timeout), (vec!["API_URL".to_string()], Duration::from_millis(2500)));
    assert_eq!(parse("[bridge]\ncommand = [\"x\"]\ntimeout = 3\n").unwrap().bridge.unwrap().timeout.as_secs(), 3);
    assert!(parse("").unwrap().bridge.is_none());
    let err = |text: &str| parse(text).unwrap_err();
    assert_eq!(err("[bridge]\ncommand = [\"x\"]\nport = 1\n"), "`bridge`: unknown key `port`");
    assert!(err("[bridge]\n").contains("`bridge.command` starts the server"));
    assert_eq!(err("[bridge]\ncommand = \"ruby s.rb\"\n"), "`bridge.command` is an array of strings");
    assert_eq!(err("[bridge]\ncommand = [\"\"]\n"), "`bridge.command` is an array of strings");
    assert_eq!(
        err("[bridge]\ncommand = [\"x\"]\nenv = [\"A-B\"]\n"),
        "`bridge.env`: `A-B` is not an environment variable's name"
    );
    assert!(err("[bridge]\ncommand = [\"x\"]\ntimeout = 0\n").contains("more than 0"));
    assert!(err("[bridge]\ncommand = [\"x\"]\ntimeout = \"1s\"\n").contains("more than 0"));
    assert!(err("[native]\n[bridge]\ncommand = [\"x\"]\n").contains("not both"));
}
