//! The helper libraries Grenat ships — `grenat/bridge` (Ruby) and
//! `grenat_bridge` (Python) — spoken to directly, line by line: the
//! manifest they describe, the JSON-RPC errors they answer, what they
//! refuse to export.
//!
//! A test whose interpreter is not installed says so and returns.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use grenat_bridge::fixture::{self, PYTHON, RUBY};
use grenat_bridge::helpers;
use serde_json::{Value as Json, json};

/// The helpers written once, in a directory of the system's temporary one.
fn lib() -> PathBuf {
    let lib = std::env::temp_dir().join(format!("grenat-bridge-helpers-tests-{}", std::process::id()));
    helpers::write(&lib).unwrap();
    lib
}

/// What the server `script` (in the language of `facet`) answers to
/// `requests`, a line each; `None` if the interpreter is missing.
fn serve(facet: &str, script: &str, requests: &[&str]) -> Option<(Vec<Json>, String)> {
    if let Some(why) = fixture::missing(facet) {
        println!("skipped: {why}");
        return None;
    }
    let (program, flag) = if facet == RUBY { ("ruby", "-e") } else { ("python3", "-c") };
    let mut child = Command::new(program)
        .args([flag, script])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(helpers::env(&lib()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    let responses = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON: {line}: {e}")))
        .collect();
    Some((responses, stderr))
}

const RUBY_SERVER: &str = r#"
require "grenat/bridge"
Grenat::Bridge.struct(:Pair, fields: {left: :int, right: "Int?"})
Grenat::Bridge.export(:all, params: {a: :string, b: :integer, c: :float, d: :boolean, e: [:Pair], f: {string: [:bool]}},
                      returns: "String?", effects: ["fs.read", "net(\"api.x.com\")"], doc: "All the types.") { |**| nil }
Grenat::Bridge.export(:hello, returns: :string, pure: true) { puts "noise"; $stdout.puts "more"; "hello" }
Grenat::Bridge.export(:nan, returns: :float) { Float::NAN }
Grenat::Bridge.run
"#;

const PYTHON_SERVER: &str = r#"
from typing import Dict, List, Optional
from grenat_bridge import export, run, struct
struct("Pair", fields={"left": int, "right": Optional[int]})
@export(effects=["fs.read", 'net("api.x.com")'], doc="All the types.")
def all(a: str, b: int, c: float, d: bool, e: List["Pair"], f: Dict[str, List[bool]]) -> Optional[str]:
    return None
@export(pure=True)
def hello() -> str:
    print("noise")
    return "hello"
@export
def nan() -> float:
    return float("nan")
run()
"#;

const REQUESTS: [&str; 9] = [
    r#"{"jsonrpc":"2.0","id":1,"method":"describe"}"#,
    r#"{"jsonrpc":"2.0","id":2,"method":"call","params":{"name":"hello","args":[]}}"#,
    r#"{"jsonrpc":"2.0","method":"call","params":{"name":"hello","args":[]}}"#,
    "{not json",
    r#"{"id":5,"method":"describe"}"#,
    r#"{"jsonrpc":"2.0","id":6,"method":"frobnicate"}"#,
    r#"{"jsonrpc":"2.0","id":7,"method":"call","params":{"name":"nothing","args":[]}}"#,
    r#"{"jsonrpc":"2.0","id":8,"method":"call","params":{"name":"hello","args":[1]}}"#,
    r#"{"jsonrpc":"2.0","id":9,"method":"call","params":{"name":"nan","args":[]}}"#,
];

fn check(facet: &str, script: &str) {
    let Some((responses, stderr)) = serve(facet, script, &REQUESTS) else { return };
    // a notification is not answered: 8 responses to 9 lines
    assert_eq!(responses.len(), 8, "{responses:#?}");
    let manifest = &responses[0]["result"];
    assert_eq!(manifest["abi"], 1);
    assert_eq!(
        manifest["structs"],
        json!([{"name": "Pair", "doc": null, "fields": [
            {"name": "left", "type": "Int", "doc": null}, {"name": "right", "type": "Int?", "doc": null}]}])
    );
    let all = &manifest["functions"][0];
    let types: Vec<&str> = all["params"].as_array().unwrap().iter().map(|p| p["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["String", "Int", "Float", "Bool", "Array(Pair)", "Hash(String, Array(Bool))"]);
    assert_eq!(
        (&all["name"], &all["symbol"], &all["returns"], &all["pure"], &all["error"], &all["doc"]),
        (
            &json!("all"),
            &json!("all"),
            &json!("String?"),
            &json!(false),
            &json!("BridgeError"),
            &json!("All the types.")
        )
    );
    assert_eq!(all["effects"], json!(["fs.read", "net(\"api.x.com\")"]));
    assert_eq!(manifest["functions"][1]["pure"], true);
    // what a function prints goes to standard error
    assert_eq!(responses[1], json!({"jsonrpc": "2.0", "id": 2, "result": "hello"}));
    assert!(stderr.contains("noise"), "{stderr}");
    let error = |i: usize| (responses[i]["id"].clone(), responses[i]["error"]["code"].as_i64().unwrap());
    assert_eq!(error(2), (Json::Null, -32700));
    assert_eq!(error(3), (json!(5), -32600));
    assert_eq!(error(4), (json!(6), -32601));
    assert_eq!(error(5), (json!(7), -32602));
    assert_eq!(responses[5]["error"]["message"], "no function `nothing` is exported");
    assert_eq!(error(6), (json!(8), -32602));
    assert_eq!(responses[6]["error"]["message"], "`hello` takes 0 argument(s)");
    // a result that is not JSON is an error of the bridge
    assert_eq!(error(7), (json!(9), -32000));
    assert_eq!(responses[7]["error"]["data"]["type"], "BridgeError");
}

#[test]
fn the_ruby_helper_speaks_json_rpc() {
    check(RUBY, RUBY_SERVER);
}

#[test]
fn the_python_helper_speaks_json_rpc() {
    check(PYTHON, PYTHON_SERVER);
}

#[test]
fn text_is_utf8_without_a_locale() {
    // `serve` gives the server no `LANG`: Ruby's default encoding is then US-ASCII
    let ruby = r#"
require "grenat/bridge"
Grenat::Bridge.export(:shout, params: {text: :string}, returns: :string) { |text:| warn "got #{text}"; text.upcase }
Grenat::Bridge.run
"#;
    let python = r#"
from grenat_bridge import export, run
@export
def shout(text: str) -> str:
    print("got " + text)
    return text.upper()
run()
"#;
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"call","params":{"name":"shout","args":["hé 你好"]}}"#;
    for (facet, script) in [(RUBY, ruby), (PYTHON, python)] {
        let raw = "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"call\",\"params\":{\"name\":\"shout\",\"args\":[\"é\"]}}";
        let Some((responses, stderr)) = serve(facet, script, &[request, raw]) else { continue };
        assert_eq!(responses[0]["result"], json!("HÉ 你好"), "{facet}: {stderr}");
        assert_eq!(responses[1]["result"], json!("É"), "{facet}: {stderr}");
        assert!(stderr.contains("got hé 你好"), "{facet}: {stderr}");
    }
}

#[test]
fn a_string_that_is_not_utf8_is_an_error_not_a_lost_response() {
    let python = r#"
from grenat_bridge import export, run
@export
def listed() -> str:
    return "a\udcffb"
run()
"#;
    let ruby = r#"
require "grenat/bridge"
Grenat::Bridge.export(:listed, returns: :string) { "a\xFFb".dup.force_encoding("UTF-8") }
Grenat::Bridge.run
"#;
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"call","params":{"name":"listed","args":[]}}"#;
    for (facet, script) in [(PYTHON, python), (RUBY, ruby)] {
        let Some((responses, stderr)) = serve(facet, script, &[request]) else { continue };
        assert_eq!(responses[0]["id"], json!(1), "{facet}: {stderr}");
        assert_eq!(responses[0]["error"]["data"]["type"], json!("BridgeError"), "{facet}");
        let message = responses[0]["error"]["message"].as_str().unwrap();
        assert!(message.starts_with("the result cannot be sent as JSON"), "{facet}: {message}");
    }
}

/// The error a server raises while it declares its functions.
fn refused(facet: &str, script: &str) -> Option<String> {
    if fixture::missing(facet).is_some() {
        return None;
    }
    let (program, flag) = if facet == RUBY { ("ruby", "-e") } else { ("python3", "-c") };
    let out = Command::new(program).args([flag, script]).envs(helpers::env(&lib())).output().unwrap();
    assert!(!out.status.success());
    Some(String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn what_cannot_cross_is_refused_when_exported() {
    let ruby = |code: &str| refused(RUBY, &format!("require 'grenat/bridge'\n{code}"));
    for (code, message) in [
        ("Grenat::Bridge.export(:Bad) { 1 }", "`Bad` is not a Grenat function name"),
        ("Grenat::Bridge.export(:f)", "`f` needs a block"),
        ("Grenat::Bridge.export(:f, effects: ['net'], pure: true) { 1 }", "a pure function has none"),
        ("Grenat::Bridge.export(:f, returns: :text) { 1 }", "unknown type `:text`"),
        ("Grenat::Bridge.export(:f, returns: {int: :int}) { 1 }", "a hash type has string keys"),
        ("Grenat::Bridge.export(:f, returns: [:int, :int]) { 1 }", "an array type has one element type"),
    ] {
        if let Some(stderr) = ruby(code) {
            assert!(stderr.contains(message), "{code}: {stderr}");
        }
    }
    let python =
        |code: &str| refused(PYTHON, &format!("from typing import Dict\nfrom grenat_bridge import export\n{code}"));
    for (code, message) in [
        ("@export(name='Bad')\ndef f() -> int: return 1", "`Bad` is not a Grenat function name"),
        ("@export(effects=['net'], pure=True)\ndef f() -> int: return 1", "a pure function has none"),
        ("@export\ndef f(x) -> int: return 1", "give the type of x"),
        ("@export\ndef f() -> bytes: return b''", "unknown type"),
        ("@export\ndef f() -> Dict[int, int]: return {}", "a dict type has string keys"),
    ] {
        if let Some(stderr) = python(code) {
            assert!(stderr.contains(message), "{code}: {stderr}");
        }
    }
}
