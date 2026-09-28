//! Native facets in an application: trusted explicitly, installed with their
//! library (here the fixture's, shipped prebuilt: nothing to compile), and
//! loaded with the declarations generated from it.

use std::path::{Path, PathBuf};

use grenat_native::fixture::{self, FACET};
use grenat_native::layout::{self, Layout};
use grenat_package::facets::install;
use grenat_package::{LoadError, load, native};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-native-facets-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A copy of the facet `sheets` in `dir`, shipping the fixture's library prebuilt.
fn facet(dir: &Path) -> PathBuf {
    let (_, installed) = fixture::installed();
    let facet = fixture::copy_facet(dir);
    let prebuilt = layout::prebuilt(FACET, &facet.join("native"));
    std::fs::create_dir_all(prebuilt.parent().unwrap()).unwrap();
    std::fs::copy(&installed.library, &prebuilt).unwrap();
    facet
}

/// An application using the facet in `dir/sheets`, with this `Facetfile` line.
fn app(dir: &Path, line: &str) -> PathBuf {
    let app = dir.join("app");
    write(&app.join("grenat.toml"), "[package]\nname = \"app\"\n");
    write(&app.join("Facetfile"), &format!("{line}\n"));
    write(&app.join("src/main.grn"), "require \"sheets\"\n\ndef main\n  puts total([1, 2]), add(1, 2)\nend\n");
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
fn a_native_facet_is_refused_unless_trusted() {
    let dir = temp_dir("untrusted");
    let facet = facet(&dir);
    let app = app(&dir, "facet \"sheets\", path: \"../sheets\"");
    let e = install(&app, false).unwrap_err();
    assert_eq!(
        e,
        "facet `sheets` ships native code (Rust), which runs outside Grenat's sandbox: \
         trust it with `facet \"sheets\", native: true` in the Facetfile"
    );
    // nothing recorded, nothing built
    assert!(!app.join("Facetfile.lock").exists());
    assert!(!Layout::new(FACET, &facet).library().exists());
}

#[test]
fn a_trusted_native_facet_is_installed_and_loaded_with_its_declarations() {
    let dir = temp_dir("trusted");
    let facet = facet(&dir);
    let app = app(&dir, "facet \"sheets\", path: \"../sheets\", native: true");
    install(&app, false).unwrap();
    let layout = Layout::new(FACET, &facet);
    assert!(layout.library().is_file() && layout.manifest().is_file() && layout.declarations().is_file());
    let bundle = load(&app.join("src/main.grn"), false).unwrap();
    assert!(bundle.sources.text.contains("native def add(a: Int, b: Int) -> Int pure\n"), "{}", bundle.sources.text);
    assert!(bundle.sources.text.contains("def total(column: Array(Int)) -> Int"));
    let libraries = native::libraries(&app).unwrap();
    assert_eq!(libraries.len(), 1);
    assert_eq!(libraries[0].facet, FACET);
    assert!(libraries[0].manifest.function("read_sheet").is_some());
    assert_eq!(native::functions(FACET, &facet), Some(libraries[0].manifest.functions.len()));

    // trust withdrawn: the program is refused, and no library is given
    write(&app.join("Facetfile"), "facet \"sheets\", path: \"../sheets\"\n");
    assert!(load_error(&app.join("src/main.grn")).starts_with("facet `sheets` ships native code (Rust)"));
    assert!(native::libraries(&app).unwrap().is_empty());

    // trusted again, but its declarations gone: install it again
    write(&app.join("Facetfile"), "facet \"sheets\", path: \"../sheets\", native: true\n");
    std::fs::remove_file(layout.declarations()).unwrap();
    assert_eq!(
        load_error(&app.join("src/main.grn")),
        "the native part of facet `sheets` is not built: run `setter install`"
    );
}

#[test]
fn a_native_facet_sees_its_own_native_functions() {
    let dir = temp_dir("own");
    let facet = facet(&dir);
    let test = facet.join("tests/lib_test.grn");
    assert_eq!(load_error(&test), "the native part of facet `sheets` is not built: run `setter install`");
    // the facet's own code is its author's: trusted without a Facetfile line
    install(&facet, false).unwrap();
    let bundle = load(&test, false).unwrap();
    assert!(bundle.sources.text.contains("native def shout(text: String) -> ~String\n"));
    assert_eq!(native::libraries(&facet).unwrap()[0].facet, FACET);
}

#[test]
fn the_native_section_of_a_manifest() {
    let manifest = grenat_package::Manifest::parse("[package]\nname = \"x\"\n[native]\n").unwrap();
    assert_eq!(manifest.native.unwrap().path, Path::new("native"));
    let manifest = grenat_package::Manifest::parse("[package]\nname = \"x\"\n[native]\npath = \"rust\"\n").unwrap();
    assert_eq!(manifest.native.unwrap().path, Path::new("rust"));
    assert!(grenat_package::Manifest::parse("[package]\nname = \"x\"\n").unwrap().native.is_none());
    let err = |text: &str| grenat_package::Manifest::parse(text).unwrap_err();
    assert_eq!(err("[package]\nname = \"x\"\n[native]\ncrate = \"n\"\n"), "`native`: unknown key `crate`");
    assert!(err("[package]\nname = \"x\"\n[native]\npath = \"/abs\"\n").contains("relative to the package"));
    assert!(err("native = 1\n[package]\nname = \"x\"\n").contains("must be a table"));
}
