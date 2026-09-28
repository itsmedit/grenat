//! The code `#[export]` and `#[derive(GrenatType)]` generate, called in
//! process through the C entry points, as Grenat calls them in a library.

use std::collections::HashMap;

use grenat_ext::abi::{self, Buffer, STATUS_ERROR, STATUS_OK, STATUS_PANIC};
use grenat_ext::manifest::{Field, Function};
use grenat_ext::{GrenatType, export};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

/// A point of the plane.
#[derive(Serialize, Deserialize, GrenatType)]
pub struct Point {
    /// Across.
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Deserialize, GrenatType)]
pub struct Path {
    pub name: String,
    pub points: Vec<Point>,
    pub next: Option<Box<Path>>,
}

/// Adds two integers.
#[export(pure)]
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// The middle of two points.
#[export(pure)]
pub fn middle(a: Point, b: Point) -> Point {
    Point { x: (a.x + b.x) / 2.0, y: (a.y + b.y) / 2.0 }
}

#[export(effects = "fs.read, net(\"api.x.com\")", error = "LookupError")]
pub fn lookup(names: Vec<String>, wanted: Option<String>) -> Result<HashMap<String, i64>, String> {
    let wanted = wanted.ok_or("nothing wanted")?;
    Ok(names.iter().filter(|n| **n == wanted).map(|n| (n.clone(), n.len() as i64)).collect())
}

#[export]
pub fn ignore(_path: Path) {}

#[export]
pub fn explode(message: String) -> bool {
    panic!("{message}")
}

/// Calls the entry point `entry` with `args`: the status and the JSON it wrote.
fn call(entry: abi::EntryFn, args: Json) -> (i32, Json) {
    let bytes = serde_json::to_vec(&args).unwrap();
    let mut out = Buffer::empty();
    // SAFETY: live bytes and a local buffer, freed by this library's `grenat_ext_free`
    unsafe {
        let status = entry(bytes.as_ptr(), bytes.len(), &mut out);
        let json = serde_json::from_slice(out.as_slice()).unwrap();
        grenat_ext::grenat_ext_free(out);
        (status, json)
    }
}

#[test]
fn entry_points_decode_call_and_encode() {
    assert_eq!(call(grenat_ext_v1_add, json!([2, 40])), (STATUS_OK, json!(42)));
    let (status, point) = call(grenat_ext_v1_middle, json!([{"x": 0.0, "y": 2.0}, {"x": 4.0, "y": 4.0}]));
    assert_eq!((status, point), (STATUS_OK, json!({"x": 2.0, "y": 3.0})));
    let (status, found) = call(grenat_ext_v1_lookup, json!([["ada", "bob", "ada"], "ada"]));
    assert_eq!((status, found), (STATUS_OK, json!({"ada": 3})));
    let path = json!({"name": "p", "points": [], "next": {"name": "q", "points": [{"x": 1, "y": 1}], "next": null}});
    assert_eq!(call(grenat_ext_v1_ignore, json!([path])), (STATUS_OK, Json::Null));
}

#[test]
fn errors_panics_and_bad_arguments_come_back_as_errors() {
    let (status, error) = call(grenat_ext_v1_lookup, json!([["ada"], null]));
    assert_eq!((status, error), (STATUS_ERROR, json!({"type": "LookupError", "message": "nothing wanted"})));
    let (status, error) = call(grenat_ext_v1_explode, json!(["the sheet is on fire"]));
    assert_eq!(status, STATUS_PANIC);
    let message = error["message"].as_str().unwrap();
    assert!(message.starts_with("the sheet is on fire (at crates/grenat_ext/tests/export.rs:"), "{message}");
    let (status, error) = call(grenat_ext_v1_add, json!([1]));
    assert_eq!(status, STATUS_ERROR);
    assert_eq!(error["type"], "ArgumentError");
    let (status, error) = call(grenat_ext_v1_add, json!([1, "two"]));
    assert_eq!(status, STATUS_ERROR);
    assert!(error["message"].as_str().unwrap().starts_with("`add`: argument `b`"), "{error}");
}

#[test]
fn the_manifest_describes_every_function_and_struct() {
    let manifest = grenat_ext::manifest();
    assert_eq!(manifest.abi, abi::ABI_VERSION);
    let names: Vec<&str> = manifest.functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["add", "explode", "ignore", "lookup", "middle"]);
    let field = |name: &str, ty: &str| Field { name: name.into(), ty: ty.into(), doc: None };
    assert_eq!(
        manifest.function("add").unwrap(),
        &Function {
            name: "add".into(),
            symbol: format!("{}add", abi::ENTRY_PREFIX),
            doc: Some("Adds two integers.".into()),
            params: vec![field("a", "Int"), field("b", "Int")],
            returns: "Int".into(),
            effects: vec![],
            pure: true,
            error: "NativeError".into(),
        }
    );
    let lookup = manifest.function("lookup").unwrap();
    assert_eq!(lookup.params, [field("names", "Array(String)"), field("wanted", "String?")]);
    assert_eq!(lookup.returns, "Hash(String, Int)");
    assert_eq!(lookup.effects, ["fs.read", "net(\"api.x.com\")"]);
    assert_eq!((lookup.pure, lookup.error.as_str()), (false, "LookupError"));
    assert_eq!(manifest.function("ignore").unwrap().returns, "Nil");
    let structs: Vec<&str> = manifest.structs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(structs, ["Path", "Point"]);
    let point = &manifest.structs[1];
    assert_eq!(point.doc.as_deref(), Some("A point of the plane."));
    assert_eq!(point.fields[0], Field { name: "x".into(), ty: "Float".into(), doc: Some("Across.".into()) });
    let path = &manifest.structs[0];
    assert_eq!(path.fields[1].ty, "Array(Point)");
    assert_eq!(path.fields[2].ty, "Path?");
}

#[test]
fn the_manifest_crosses_the_boundary_as_json() {
    let mut out = Buffer::empty();
    // SAFETY: a local buffer, freed by this library
    let (status, json): (i32, Json) = unsafe {
        let status = grenat_ext::grenat_ext_manifest(&mut out);
        let json = serde_json::from_slice(out.as_slice()).unwrap();
        grenat_ext::grenat_ext_free(out);
        (status, json)
    };
    assert_eq!(status, STATUS_OK);
    assert_eq!(serde_json::from_value::<grenat_ext::manifest::Manifest>(json).unwrap(), grenat_ext::manifest());
    assert_eq!(grenat_ext::grenat_ext_abi_version(), abi::ABI_VERSION);
}
