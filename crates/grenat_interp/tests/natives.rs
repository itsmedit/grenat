//! `native def`: Grenat code calling the Rust functions of a real native
//! facet (the fixture `sheets`, built by cargo and installed once).

use std::sync::{Arc, Mutex};

use grenat_interp::{Options, Output, RuntimeError, run_main};
use grenat_native::fixture;

/// Runs `code` after the facet's generated declarations, with its library.
fn run_with(
    code: &str,
    natives: Vec<grenat_native::Installed>,
    declarations: &str,
) -> (Result<(), RuntimeError>, String) {
    let src = format!("{declarations}\n{code}");
    let parsed = grenat_parser::parse(&src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}\n{src}", parsed.diagnostics);
    let out = Arc::new(Mutex::new(String::new()));
    let options = Options { output: Output::Capture(out.clone()), natives, ..Options::default() };
    let result = run_main(&parsed.program, Vec::new(), options).map(drop);
    let output = out.lock().unwrap().clone();
    (result, output)
}

fn run(code: &str) -> (Result<(), RuntimeError>, String) {
    let (_, installed) = fixture::installed();
    run_with(code, vec![installed.clone()], &fixture::declarations())
}

fn ok(code: &str) -> String {
    match run(code) {
        (Ok(()), output) => output,
        (Err(e), output) => panic!("{e:?}\noutput:\n{output}"),
    }
}

fn error(code: &str) -> RuntimeError {
    match run(code) {
        (Err(e), _) => e,
        (Ok(()), output) => panic!("expected an error, got:\n{output}"),
    }
}

#[test]
fn native_functions_are_called_with_their_types() {
    let out = ok("\
puts add(40, 2)
puts sum([1, 2, 3])
puts mean([1.0, 2.0])
p mean([])
p find([\"a\", \"b\"], \"b\")
p find([\"a\"], \"z\")
counts = count_words(\"a b a\")
puts counts[\"a\"], counts[\"b\"]
cells = cells([[\"a\", \"bb\"], [\"ccc\"]])
puts cells.size, cells[1].text, cells[2].row
best = longest(cells)
puts best.text
p longest([])
");
    assert_eq!(out, "42\n6\n1.5\nnil\n1\nnil\n2\n1\n3\nbb\n1\nccc\nnil\n");
}

#[test]
fn a_native_result_is_untrusted_unless_pure() {
    let out = ok("\
puts shout(\"hi\").tainted?, shout(\"hi\").trust!
puts add(1, 2).tainted?, cells([[\"x\"]]).first.text.tainted?
puts count_words(shout(\"a a\")).tainted?
");
    assert_eq!(out, "true\nHI\nfalse\nfalse\ntrue\n");
    // an untrusted value cannot reach a function with a dangerous effect
    let e = error("fetch(shout(\"example.com\"))\n");
    assert_eq!(e.ty, "TaintError");
    assert!(e.message.contains("`fetch` (effect `net`)"), "{}", e.message);
    assert_eq!(ok("puts fetch(shout(\"example.com\").trust!).trust!\n"), "fetched EXAMPLE.COM\n");
}

#[test]
fn the_effects_of_a_native_function_are_enforced_on_its_callers() {
    let dir = std::env::temp_dir().join(format!("grenat-natives-sheet-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sheet = dir.join("sheet.csv");
    std::fs::write(&sheet, "a, b\nc, d\n").unwrap();
    let path = sheet.display();
    let out = ok(&format!(
        "def load(path: String) -> Array(Array(String)) uses fs.read\n  read_sheet(path).trust!\nend\nputs load(\"{path}\").map {{ |row| row.join(\"|\") }}.join(\";\")\n"
    ));
    assert_eq!(out, "a|b;c|d\n");
    let e = error(&format!("def load uses net\n  read_sheet(\"{path}\")\nend\nload\n"));
    assert_eq!(e.ty, "CapabilityError");
    assert_eq!(e.message, "`fs.read` is not allowed by `load` (uses net)");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn errors_and_panics_are_grenat_errors() {
    let e = error("ratio(1.0, 0.0)\n");
    assert_eq!((e.ty.as_str(), e.message.as_str()), ("NativeError", "cannot divide 1 by zero"));
    assert_eq!(e.trace[0].0, "ratio");
    let e = error("read_sheet(\"/nowhere/sheet.csv\")\n");
    assert_eq!(e.ty, "SheetError");
    let e = error("explode(\"on fire\")\n");
    assert_eq!(e.ty, "NativeError");
    assert!(e.message.starts_with("`explode` panicked: on fire (at src/lib.rs:"), "{}", e.message);
    // rescued, the program goes on: the library still answers
    let out = ok("\
begin
  read_sheet(\"/nowhere\")
rescue SheetError => e
  puts \"no sheet\"
end
begin
  explode(\"boom\")
rescue NativeError => e
  puts e.message.start_with?(\"`explode` panicked: boom\")
end
puts add(1, 1)
");
    assert_eq!(out, "no sheet\ntrue\n2\n");
}

#[test]
fn no_secret_is_handed_to_native_code() {
    let e = error("mock_credentials({\"api\" => {\"key\" => \"s3cr3t\"}})\nshout(Credentials.fetch(:api, :key))\n");
    assert_eq!(e.ty, "SecretError");
    assert!(!e.message.contains("s3cr3t"), "{}", e.message);
}

#[test]
fn a_declaration_without_its_library_fails_when_called() {
    let (_, installed) = fixture::installed();
    let decl = "native def add(a: Int, b: Int) -> Int pure";
    let (result, _) = run_with("puts add(1, 2)\n", Vec::new(), decl);
    let e = result.unwrap_err();
    assert_eq!(e.ty, "NativeError");
    assert!(e.message.starts_with("no native library provides `add`"), "{}", e.message);
    // a library of another ABI version is refused when it loads
    let old = grenat_native::Installed { library: fixture::old_abi_library().to_path_buf(), ..installed.clone() };
    let e = run_with("puts add(1, 2)\n", vec![old], decl).0.unwrap_err();
    assert_eq!(e.ty, "NativeError");
    assert!(e.message.contains("version 0 of Grenat's native ABI"), "{}", e.message);
    // another type than the library's: not bound to it
    let wrong = "native def add(a: Int, b: Int) -> String pure";
    let e = run_with("puts add(1, 2)\n", vec![installed.clone()], wrong).0.unwrap_err();
    assert_eq!(e.ty, "NativeError");
    assert_eq!(
        e.message,
        "`add` is not declared as facet `sheets` exports it (`native def add(a: Int, b: Int) -> Int pure`): a \
         `native def` is written by `setter install`, not by hand"
    );
}

#[test]
fn a_declaration_written_by_hand_cannot_drop_effects_or_taint() {
    let dir = std::env::temp_dir().join(format!("grenat-natives-forged-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sheet = dir.join("sheet.csv");
    std::fs::write(&sheet, "secret, cells\n").unwrap();
    let (_, installed) = fixture::installed();
    let forged = "native def read_sheet(path: String) -> Array(Array(String)) pure";
    let code = format!("def load uses net\n  read_sheet(\"{}\")\nend\nv = load\nputs v, v.tainted?\n", sheet.display());
    let (result, output) = run_with(&code, vec![installed.clone()], forged);
    let e = result.unwrap_err();
    assert_eq!(e.ty, "NativeError", "{output}");
    assert!(e.message.starts_with("`read_sheet` is not declared as facet `sheets` exports it"), "{}", e.message);
    assert_eq!(output, "");
    // a function of the library whose declaration drops an effect, or its `~`
    for (forged, call) in [
        ("native def read_sheet(path: String) -> ~Array(Array(String))", "read_sheet(\"x\")"),
        ("native def shout(text: String) -> String pure", "shout(\"x\")"),
        ("native def add(a: Int, b: Int) -> ~Int uses net", "add(1, 2)"),
        ("native def add(a: Int, c: Int) -> Int pure", "add(1, 2)"),
    ] {
        let (result, _) = run_with(&format!("{call}\n"), vec![installed.clone()], forged);
        assert!(result.unwrap_err().message.contains("is not declared as facet `sheets` exports it"), "{forged}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_facet_wraps_its_native_functions_in_grenat() {
    let (facet, _) = fixture::installed();
    let lib = std::fs::read_to_string(facet.join("src/lib.grn")).unwrap();
    assert_eq!(ok(&format!("{lib}\nputs total([1, 2, 3])\n")), "6\n");
}
