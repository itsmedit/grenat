//! A real facet library (the fixture `sheets`), built by cargo, installed,
//! loaded and called through the ABI.

use std::path::Path;

use grenat_native::fixture::{self, FACET};
use grenat_native::layout::{self, Layout};
use grenat_native::{Library, Outcome, Registry, install};
use serde_json::{Value as Json, json};

fn call(name: &str, args: Json) -> Outcome {
    let (_, installed) = fixture::installed();
    let library = Library::open(&installed.library).unwrap();
    library.call(name, serde_json::to_vec(&args).unwrap().as_slice()).unwrap()
}

#[test]
fn the_manifest_is_read_from_the_library() {
    let (facet, installed) = fixture::installed();
    let manifest = &installed.manifest;
    let names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "add",
            "cells",
            "count_words",
            "explode",
            "fetch",
            "find",
            "longest",
            "mean",
            "ratio",
            "read_sheet",
            "shout",
            "sum"
        ]
    );
    let read = manifest.function("read_sheet").unwrap();
    assert_eq!(
        (read.returns.as_str(), read.effects.as_slice(), read.pure),
        ("Array(Array(String))", &["fs.read".to_string()][..], false)
    );
    assert_eq!(read.error, "SheetError");
    assert_eq!(read.doc.as_deref(), Some("Reads a sheet: a line per row, cells separated by commas."));
    assert_eq!(manifest.structs[0].name, "Cell");
    // what setter wrote into the facet
    let layout = Layout::new(FACET, facet);
    assert!(layout.library().is_file());
    let written: grenat_native::Manifest =
        serde_json::from_str(&std::fs::read_to_string(layout.manifest()).unwrap()).unwrap();
    assert_eq!(&written, manifest);
    assert_eq!(grenat_native::Installed::read(FACET, facet).unwrap(), *installed);
}

#[test]
fn the_declarations_are_grenat_code() {
    let text = fixture::declarations();
    for line in [
        "struct Cell\n  ## Its row, from 0.\n  row: Int\n  col: Int\n  text: String\nend\n",
        "## Adds two integers.\nnative def add(a: Int, b: Int) -> Int pure\n",
        "native def read_sheet(path: String) -> ~Array(Array(String)) uses fs.read\n",
        "native def shout(text: String) -> ~String\n",
        "native def find(words: Array(String), word: String) -> Int? pure\n",
        "native def count_words(text: String) -> Hash(String, Int) pure\n",
        "native def longest(cells: Array(Cell)) -> Cell? pure\n",
        "native def fetch(url: String) -> ~String uses net\n",
    ] {
        assert!(text.contains(line), "no {line:?} in:\n{text}");
    }
    let parsed = grenat_parser::parse(&text);
    assert!(parsed.diagnostics.is_empty(), "{:?}\n{text}", parsed.diagnostics);
}

#[test]
fn functions_are_called_with_json() {
    assert_eq!(call("add", json!([40, 2])), Outcome::Returned(json!(42)));
    assert_eq!(call("find", json!([["a", "b"], "b"])), Outcome::Returned(json!(1)));
    assert_eq!(call("find", json!([["a"], "z"])), Outcome::Returned(Json::Null));
    assert_eq!(call("count_words", json!(["a b a"])), Outcome::Returned(json!({"a": 2, "b": 1})));
    assert_eq!(call("mean", json!([[1.0, 2.0]])), Outcome::Returned(json!(1.5)));
    let cells = call("cells", json!([[["a", "bb"]]]));
    assert_eq!(
        cells,
        Outcome::Returned(json!([{"row": 0, "col": 0, "text": "a"}, {"row": 0, "col": 1, "text": "bb"}]))
    );
    let longest = call("longest", json!([[{"row": 0, "col": 0, "text": "a"}, {"row": 1, "col": 0, "text": "ccc"}]]));
    assert_eq!(longest, Outcome::Returned(json!({"row": 1, "col": 0, "text": "ccc"})));
}

#[test]
fn errors_and_panics_are_outcomes_not_crashes() {
    assert_eq!(
        call("ratio", json!([1.0, 0.0])),
        Outcome::Raised { ty: "NativeError".into(), message: "cannot divide 1 by zero".into() }
    );
    match call("read_sheet", json!(["/nowhere/sheet.csv"])) {
        Outcome::Raised { ty, message } => {
            assert_eq!(ty, "SheetError");
            assert!(message.starts_with("cannot read /nowhere/sheet.csv"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    match call("explode", json!(["on fire"])) {
        Outcome::Panicked(message) => assert!(message.starts_with("on fire (at src/lib.rs:"), "{message}"),
        other => panic!("{other:?}"),
    }
    // the library still answers after a panic
    assert_eq!(call("add", json!([1, 1])), Outcome::Returned(json!(2)));
    match call("add", json!([1, "one"])) {
        Outcome::Raised { ty, .. } => assert_eq!(ty, "TypeError"),
        other => panic!("{other:?}"),
    }
    let (_, installed) = fixture::installed();
    let library = Library::open(&installed.library).unwrap();
    assert!(library.call("nothing", b"[]").unwrap_err().contains("exports no function `nothing`"));
}

#[test]
fn a_library_is_loaded_once() {
    let (_, installed) = fixture::installed();
    let a = Library::open(&installed.library).unwrap();
    let b = Library::open(&installed.library).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
}

#[test]
fn another_abi_version_is_refused() {
    let e = Library::open(fixture::old_abi_library()).unwrap_err();
    assert!(e.contains("was built for version 0 of Grenat's native ABI"), "{e}");
    assert!(e.contains("this Grenat speaks version 1"), "{e}");
    let not_a_library = std::env::temp_dir().join(format!("grenat-not-a-library-{}.so", std::process::id()));
    std::fs::write(&not_a_library, "text").unwrap();
    assert!(Library::open(&not_a_library).unwrap_err().starts_with("cannot load"));
    assert!(Library::open(Path::new("/nowhere/libx.so")).unwrap_err().starts_with("cannot find"));
}

#[test]
fn the_registry_calls_by_name_and_loads_lazily() {
    let (_, installed) = fixture::installed();
    let registry = Registry::new(std::slice::from_ref(installed));
    assert_eq!(registry.function("add").map(|(facet, f)| (facet, f.pure)), Some((FACET, true)));
    assert_eq!(registry.call("sum", b"[[1,2,3]]").unwrap(), Outcome::Returned(json!(6)));
    let e = registry.call("nowhere", b"[]").unwrap_err();
    assert!(e.starts_with("no native library provides `nowhere`"), "{e}");
    // a library missing on disk fails at the first call, not before
    let mut gone = installed.clone();
    gone.library = "/nowhere/libsheets.so".into();
    let registry = Registry::new(&[gone]);
    assert!(registry.function("add").is_some());
    assert!(registry.call("add", b"[1,2]").unwrap_err().starts_with("facet `sheets`: cannot find"));
}

#[test]
fn a_prebuilt_library_is_used_instead_of_building() {
    let (_, installed) = fixture::installed();
    let dir = std::env::temp_dir().join(format!("grenat-native-prebuilt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let facet = fixture::copy_facet(&dir);
    // no crate to build: only the library, for this platform
    std::fs::remove_file(facet.join("native/Cargo.toml")).unwrap();
    let prebuilt = layout::prebuilt(FACET, &facet.join("native"));
    std::fs::create_dir_all(prebuilt.parent().unwrap()).unwrap();
    std::fs::copy(&installed.library, &prebuilt).unwrap();
    let done = install(FACET, &facet, Path::new("native"), None).unwrap();
    assert_eq!(done.manifest, installed.manifest);
    assert!(Layout::new(FACET, &facet).declarations().is_file());
    // without either, the crate is missing
    std::fs::remove_file(&prebuilt).unwrap();
    let e = install(FACET, &facet, Path::new("native"), None).unwrap_err();
    assert!(e.starts_with("no Cargo.toml in"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}
