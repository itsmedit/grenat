//! The language server, message by message.

use std::path::{Path, PathBuf};

use grenat_lsp::{Server, read_message, serve, write_message};
use serde_json::{Value as Json, json};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-lsp-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn request(server: &mut Server, id: u64, method: &str, params: Json) -> Json {
    let reply = server.handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    reply.messages.into_iter().find(|m| m["id"] == id).expect("a response")
}

/// The diagnostics published by a notification, by file name.
fn notify(server: &mut Server, method: &str, params: Json) -> Vec<(String, Vec<Json>)> {
    let reply = server.handle(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    reply
        .messages
        .iter()
        .filter(|m| m["method"] == "textDocument/publishDiagnostics")
        .map(|m| {
            let uri = m["params"]["uri"].as_str().unwrap();
            let name = uri.rsplit('/').next().unwrap().to_string();
            (name, m["params"]["diagnostics"].as_array().unwrap().clone())
        })
        .collect()
}

fn open(server: &mut Server, path: &Path, text: &str) -> Vec<(String, Vec<Json>)> {
    notify(server, "textDocument/didOpen", json!({"textDocument": {"uri": uri(path), "languageId": "grenat", "version": 1, "text": text}}))
}

fn change(server: &mut Server, path: &Path, text: &str) -> Vec<(String, Vec<Json>)> {
    notify(server, "textDocument/didChange", json!({"textDocument": {"uri": uri(path), "version": 2}, "contentChanges": [{"text": text}]}))
}

fn at(path: &Path, line: u32, character: u32) -> Json {
    json!({"textDocument": {"uri": uri(path)}, "position": {"line": line, "character": character}})
}

const LIB: &str = "## Doubles a number.\ndef double(n: Int) -> Int = n * 2\n";

#[test]
fn a_session() {
    let dir = temp_dir("session");
    let (main, lib) = (dir.join("main.grn"), dir.join("lib.grn"));
    std::fs::write(&lib, LIB).unwrap();
    std::fs::write(&main, "").unwrap();
    let mut server = Server::new();

    let init = request(&mut server, 1, "initialize", json!({"capabilities": {}}));
    let capabilities = &init["result"]["capabilities"];
    assert_eq!(capabilities["textDocumentSync"]["change"], 1);
    for provider in ["documentFormattingProvider", "hoverProvider", "definitionProvider", "documentSymbolProvider"] {
        assert_eq!(capabilities[provider], true, "{provider}");
    }
    assert!(notify(&mut server, "initialized", json!({})).is_empty());

    // an error, where it is
    let published = open(&mut server, &main, "require \"./lib\"\n\ndef main\n  puts nope\nend\n");
    assert_eq!(published.len(), 1);
    let (file, diagnostics) = &published[0];
    assert_eq!(file, "main.grn");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["code"], "E0100");
    assert_eq!(diagnostics[0]["range"]["start"], json!({"line": 3, "character": 7}));
    assert_eq!(diagnostics[0]["range"]["end"], json!({"line": 3, "character": 11}));

    // fixed: no diagnostic left
    let fixed = "require \"./lib\"\n\ndef main\n  puts double(2)\nend\n";
    assert_eq!(change(&mut server, &main, fixed), [("main.grn".to_string(), Vec::new())]);

    // hover and go to definition, in the required file
    let hover = request(&mut server, 2, "textDocument/hover", at(&main, 3, 9));
    let value = hover["result"]["contents"]["value"].as_str().unwrap();
    assert_eq!(value, "```ruby\ndef double(n: Int) -> Int = n * 2\n```\n\nDoubles a number.");
    let definition = request(&mut server, 3, "textDocument/definition", at(&main, 3, 9));
    assert_eq!(definition["result"]["uri"], uri(&lib));
    assert_eq!(definition["result"]["range"]["start"], json!({"line": 1, "character": 4}));
    let nothing = request(&mut server, 4, "textDocument/hover", at(&main, 1, 0));
    assert_eq!(nothing["result"], Json::Null);

    // symbols of the document
    let symbols = request(&mut server, 5, "textDocument/documentSymbol", json!({"textDocument": {"uri": uri(&main)}}));
    let symbols = symbols["result"].as_array().unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!((symbols[0]["name"].as_str(), symbols[0]["kind"].as_u64()), (Some("main"), Some(12)));
    assert_eq!(symbols[0]["selectionRange"]["start"], json!({"line": 2, "character": 4}));

    // an unsaved change of the library shows in the program that requires it
    let published = open(&mut server, &lib, "def triple(n: Int) -> Int = n * 3\n");
    let main_diags = &published.iter().find(|(f, _)| f == "main.grn").unwrap().1;
    assert_eq!(main_diags.len(), 1);
    assert!(main_diags[0]["message"].as_str().unwrap().contains("double"), "{main_diags:?}");
    let closed = notify(&mut server, "textDocument/didClose", json!({"textDocument": {"uri": uri(&lib)}}));
    assert!(closed.contains(&("lib.grn".to_string(), Vec::new())));
    assert!(closed.contains(&("main.grn".to_string(), Vec::new())), "{closed:?}");

    // formatting
    change(&mut server, &main, "require \"./lib\"\ndef main\nputs double( 2 )\nend\n");
    let edits = request(&mut server, 6, "textDocument/formatting", json!({"textDocument": {"uri": uri(&main)}, "options": {}}));
    let edits = edits["result"].as_array().unwrap();
    assert_eq!(edits[0]["newText"], "require \"./lib\"\ndef main\n  puts double(2)\nend\n");
    assert_eq!(edits[0]["range"]["end"], json!({"line": 4, "character": 0}));
    change(&mut server, &main, "def main(\n");
    let edits = request(&mut server, 7, "textDocument/formatting", json!({"textDocument": {"uri": uri(&main)}, "options": {}}));
    assert_eq!(edits["result"], Json::Null);

    // errors
    let unknown = request(&mut server, 8, "textDocument/rename", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);
    let closed = request(&mut server, 9, "textDocument/hover", at(&dir.join("other.grn"), 0, 0));
    assert_eq!(closed["error"]["code"], -32602);

    assert_eq!(request(&mut server, 10, "shutdown", Json::Null)["result"], Json::Null);
    assert_eq!(server.handle(&json!({"jsonrpc": "2.0", "method": "exit"})).exit, Some(0));
}

#[test]
fn syntax_errors_and_unsaved_documents() {
    let dir = temp_dir("syntax");
    let path = dir.join("draft.grn");
    let mut server = Server::new();
    // never saved: not on disk
    let published = open(&mut server, &path, "def f(n: Int) -> Int = n +\n");
    assert_eq!(published[0].0, "draft.grn");
    let diagnostics = &published[0].1;
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["range"]["start"]["line"], 1);
    let published = change(&mut server, &path, "## Hello.\ndef f(n: Int) -> Int = n + 1\nf(2)\n");
    assert!(published[0].1.is_empty(), "{published:?}");
    let hover = request(&mut server, 1, "textDocument/hover", at(&path, 2, 0));
    assert!(hover["result"]["contents"]["value"].as_str().unwrap().ends_with("Hello."));
    // exit without shutdown
    assert_eq!(server.handle(&json!({"jsonrpc": "2.0", "method": "exit"})).exit, Some(1));
}

#[test]
fn messages_are_framed_over_a_stream() {
    let mut input = Vec::new();
    for message in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
        json!({"jsonrpc": "2.0", "method": "exit"}),
    ] {
        write_message(&mut input, &message).unwrap();
    }
    let mut output = Vec::new();
    assert_eq!(serve(&input[..], &mut output), 0);
    let mut reader = &output[..];
    let first = read_message(&mut reader).unwrap();
    assert_eq!(first["id"], 1);
    assert_eq!(first["result"]["serverInfo"]["name"], "grenat");
    assert_eq!(read_message(&mut reader).unwrap()["id"], 2);
    assert!(read_message(&mut reader).is_none());
    // the input ends without `exit`
    assert_eq!(serve(&b""[..], &mut Vec::new()), 1);
}

#[test]
fn macros_are_expanded_and_documented() {
    let dir = temp_dir("macros");
    let path = dir.join("m.grn");
    let text = "## Makes a constant.\nmacro constant(name, value)\n  def {{name}} = {{value}}\nend\n\nconstant :answer, 42\nputs answer\nputs missing\n";
    std::fs::write(&path, text).unwrap();
    let mut server = Server::new();
    let published = open(&mut server, &path, text);
    let diagnostics = &published[0].1;
    // `answer` exists (expanded); `missing` does not
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["range"]["start"]["line"], 7);
    let hover = request(&mut server, 1, "textDocument/hover", at(&path, 5, 3));
    assert_eq!(hover["result"]["contents"]["value"], "```ruby\nmacro constant(name, value)\n```\n\nMakes a constant.");
}
