//! The server: open documents, and the answer to each message.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use crate::analysis::Analysis;
use crate::navigation::{definitions, hover, lookup, name_at};
use crate::position::LineIndex;
use crate::transport::{read_message, write_message};
use crate::uri;

const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

#[derive(Default)]
pub struct Server {
    /// Open documents: their text in the editor.
    documents: HashMap<PathBuf, String>,
    /// The analysis of each open document's program.
    analyses: HashMap<PathBuf, Analysis>,
    shutting_down: bool,
}

/// What handling a message leads to.
pub struct Reply {
    /// Responses and notifications to send, in order.
    pub messages: Vec<Json>,
    /// The process exit code, after `exit`.
    pub exit: Option<i32>,
}

impl Server {
    pub fn new() -> Server {
        Server::default()
    }

    pub fn handle(&mut self, message: &Json) -> Reply {
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        let mut reply = Reply { messages: Vec::new(), exit: None };
        let result = match method {
            "initialize" => Ok(capabilities()),
            "shutdown" => {
                self.shutting_down = true;
                Ok(Json::Null)
            }
            "exit" => {
                reply.exit = Some(if self.shutting_down { 0 } else { 1 });
                return reply;
            }
            "textDocument/didOpen" => {
                let (path, text) = (document_path(params), params["textDocument"]["text"].as_str());
                if let (Some(path), Some(text)) = (path, text) {
                    self.documents.insert(path, text.to_string());
                }
                reply.messages = self.refresh();
                return reply;
            }
            "textDocument/didChange" => {
                let text = params["contentChanges"].as_array().and_then(|c| c.last()).and_then(|c| c["text"].as_str());
                if let (Some(path), Some(text)) = (document_path(params), text) {
                    self.documents.insert(path, text.to_string());
                }
                reply.messages = self.refresh();
                return reply;
            }
            "textDocument/didSave" => {
                reply.messages = self.refresh();
                return reply;
            }
            "textDocument/didClose" => {
                if let Some(path) = document_path(params) {
                    self.documents.remove(&path);
                    self.analyses.remove(&path);
                    reply.messages.push(diagnostics(&path, Vec::new()));
                }
                reply.messages.extend(self.refresh());
                return reply;
            }
            "textDocument/formatting" => self.formatting(params),
            "textDocument/hover" => self.hover(params),
            "textDocument/definition" => self.definition(params),
            "textDocument/documentSymbol" => self.symbols(params),
            _ if message.get("id").is_none() => return reply,
            other => Err((METHOD_NOT_FOUND, format!("unknown method `{other}`"))),
        };
        if let Some(id) = message.get("id") {
            reply.messages.push(match result {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err((code, message)) => {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
                }
            });
        }
        reply
    }

    /// Checks every open document's program again (a change in one file may
    /// matter to another) and publishes the diagnostics.
    fn refresh(&mut self) -> Vec<Json> {
        let mut paths: Vec<PathBuf> = self.documents.keys().cloned().collect();
        paths.sort();
        let mut messages = Vec::new();
        for path in paths {
            let analysis = Analysis::of(&path, &self.documents);
            messages.push(diagnostics(&path, analysis.lsp_diagnostics(&path)));
            self.analyses.insert(path, analysis);
        }
        messages
    }

    fn document(&self, params: &Json) -> Result<(PathBuf, &String), (i64, String)> {
        let path = document_path(params).ok_or((INVALID_PARAMS, "expected a `file://` document".to_string()))?;
        match self.documents.get(&path) {
            Some(text) => Ok((path, text)),
            None => Err((INVALID_PARAMS, format!("{} is not open", path.display()))),
        }
    }

    fn formatting(&self, params: &Json) -> Result<Json, (i64, String)> {
        let (_, text) = self.document(params)?;
        match grenat_fmt::format(text) {
            Ok(formatted) if formatted == *text => Ok(json!([])),
            Ok(formatted) => {
                let range = LineIndex::new(text).range(0, text.len());
                Ok(json!([{"range": range, "newText": formatted}]))
            }
            // a document that does not parse is left alone; its errors are shown
            Err(_) => Ok(Json::Null),
        }
    }

    /// The definition of the name under the cursor, and the analysis it comes from.
    fn under_cursor<R>(
        &self,
        params: &Json,
        f: impl FnOnce(&Analysis, &crate::navigation::Definition) -> R,
    ) -> Option<R> {
        let (path, text) = self.document(params).ok()?;
        let offset = LineIndex::new(text).offset(&params["position"]);
        let name = name_at(text, offset)?;
        let analysis = self.analyses.get(&path)?;
        let defs = definitions(analysis.program.as_ref()?);
        lookup(&defs, &name).map(|def| f(analysis, def))
    }

    fn hover(&self, params: &Json) -> Result<Json, (i64, String)> {
        self.document(params)?;
        let text = self.under_cursor(params, |analysis, def| hover(def, &analysis.sources));
        Ok(text.map_or(Json::Null, |value| json!({"contents": {"kind": "markdown", "value": value}})))
    }

    fn definition(&self, params: &Json) -> Result<Json, (i64, String)> {
        self.document(params)?;
        let location = self.under_cursor(params, |analysis, def| {
            let (file, span) = analysis.sources.locate(def.name_span);
            let index = LineIndex::new(analysis.sources.text_of(file));
            json!({"uri": uri::from_path(Path::new(&file.path)), "range": index.range(span.start as usize, span.end as usize)})
        });
        Ok(location.unwrap_or(Json::Null))
    }

    fn symbols(&self, params: &Json) -> Result<Json, (i64, String)> {
        let (path, _) = self.document(params)?;
        let Some(analysis) = self.analyses.get(&path) else { return Ok(json!([])) };
        let (Some(program), Some(file)) = (analysis.program.as_ref(), analysis.file(&path)) else {
            return Ok(json!([]));
        };
        let index = LineIndex::new(analysis.sources.text_of(file));
        let symbols: Vec<Json> = definitions(program)
            .iter()
            .filter(|def| def.top_level && analysis.sources.locate(def.span).0 == file)
            .map(|def| {
                let local = |span| analysis.sources.locate(span).1;
                let (span, name) = (local(def.span), local(def.name_span));
                json!({
                    "name": def.name,
                    "kind": def.kind,
                    "range": index.range(span.start as usize, span.end as usize),
                    "selectionRange": index.range(name.start as usize, name.end as usize),
                })
            })
            .collect();
        Ok(json!(symbols))
    }
}

fn capabilities() -> Json {
    json!({
        "capabilities": {
            "textDocumentSync": {"openClose": true, "change": 1, "save": {"includeText": false}},
            "documentFormattingProvider": true,
            "hoverProvider": true,
            "definitionProvider": true,
            "documentSymbolProvider": true,
        },
        "serverInfo": {"name": "grenat", "version": env!("CARGO_PKG_VERSION")},
    })
}

fn document_path(params: &Json) -> Option<PathBuf> {
    uri::to_path(params["textDocument"]["uri"].as_str()?)
}

fn diagnostics(path: &Path, diagnostics: Vec<Json>) -> Json {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {"uri": uri::from_path(path), "diagnostics": diagnostics},
    })
}

/// Serves `input` until `exit` (or the end of the input); returns the exit code.
pub fn serve(mut input: impl BufRead, mut output: impl Write) -> i32 {
    let mut server = Server::new();
    while let Some(message) = read_message(&mut input) {
        let reply = server.handle(&message);
        for message in &reply.messages {
            if write_message(&mut output, message).is_err() {
                return 1;
            }
        }
        if let Some(code) = reply.exit {
            return code;
        }
    }
    1
}
