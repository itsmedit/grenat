//! The server side: JSON-RPC messages answered for a [`Host`] that offers
//! tools. Stateless — no session, no server-sent stream: each message gets
//! its response, which suits a POST per message (streamable HTTP).

use serde_json::{Value as Json, json};

use crate::Tool;

/// What a tool call gave: its text, its value when it is a JSON object
/// (`structuredContent`), and whether the tool reports an error.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub text: String,
    pub structured: Option<Json>,
    pub is_error: bool,
}

/// What a server offers.
pub trait Host {
    /// The server's name (`serverInfo`).
    fn name(&self) -> String;
    fn tools(&mut self) -> Vec<Tool>;
    /// Calls `name` with `arguments`; `Err` when there is no such tool or the
    /// arguments do not fit its schema (a protocol error, not a tool error).
    fn call(&mut self, name: &str, arguments: &Json) -> Result<Answer, String>;
}

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// The response to a message's bytes; `None` for a notification.
pub fn handle_body(host: &mut dyn Host, body: &[u8]) -> Option<Json> {
    match serde_json::from_slice::<Json>(body) {
        Ok(message) => handle(host, &message),
        Err(e) => Some(error(Json::Null, PARSE_ERROR, &format!("invalid JSON: {e}"))),
    }
}

/// The response to `message`; `None` for a notification.
pub fn handle(host: &mut dyn Host, message: &Json) -> Option<Json> {
    let Some(method) = message.get("method").and_then(Json::as_str) else {
        return Some(error(
            message.get("id").cloned().unwrap_or(Json::Null),
            INVALID_REQUEST,
            "not a JSON-RPC request",
        ));
    };
    // a notification (no id) gets no response
    let id = message.get("id")?.clone();
    let params = message.get("params").cloned().unwrap_or(Json::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": crate::PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": host.name(), "version": env!("CARGO_PKG_VERSION")},
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": host.tools().iter().map(tool_json).collect::<Vec<_>>()})),
        "tools/call" => match params.get("name").and_then(Json::as_str) {
            Some(name) => {
                let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                host.call(name, &arguments).map(|answer| answer_json(&answer)).map_err(|e| (INVALID_PARAMS, e))
            }
            None => Err((INVALID_PARAMS, "`tools/call` expects a tool `name`".into())),
        },
        other => Err((METHOD_NOT_FOUND, format!("unknown method `{other}`"))),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => error(id, code, &message),
    })
}

fn error(id: Json, code: i64, message: &str) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_json(tool: &Tool) -> Json {
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": tool.input_schema,
        "annotations": {"readOnlyHint": tool.read_only, "destructiveHint": tool.destructive},
    })
}

fn answer_json(answer: &Answer) -> Json {
    let mut result = json!({"content": [{"type": "text", "text": answer.text}], "isError": answer.is_error});
    if let Some(structured) = &answer.structured {
        result["structuredContent"] = structured.clone();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Host for Echo {
        fn name(&self) -> String {
            "echo".into()
        }

        fn tools(&mut self) -> Vec<Tool> {
            vec![Tool {
                name: "echo".into(),
                description: "Echoes.".into(),
                input_schema: json!({"type": "object"}),
                read_only: true,
                destructive: false,
            }]
        }

        fn call(&mut self, name: &str, arguments: &Json) -> Result<Answer, String> {
            match (name, arguments.get("text")) {
                ("echo", Some(Json::String(t))) if t == "boom" => {
                    Ok(Answer { text: "Error: boom".into(), structured: None, is_error: true })
                }
                ("echo", Some(text)) => {
                    Ok(Answer { text: text.to_string(), structured: Some(json!({"text": text})), is_error: false })
                }
                ("echo", None) => Err("`text` is missing".into()),
                _ => Err(format!("unknown tool `{name}`")),
            }
        }
    }

    fn ask(message: Json) -> Option<Json> {
        handle(&mut Echo, &message)
    }

    #[test]
    fn initializes_and_lists_its_tools() {
        let init = ask(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "echo");
        assert_eq!(init["result"]["protocolVersion"], crate::PROTOCOL_VERSION);
        assert!(init["result"]["capabilities"]["tools"].is_object());
        let list = ask(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        assert_eq!(list["id"], 2);
        assert_eq!(list["result"]["tools"][0]["name"], "echo");
        assert_eq!(list["result"]["tools"][0]["annotations"]["readOnlyHint"], true);
        assert_eq!(ask(json!({"jsonrpc": "2.0", "id": 3, "method": "ping"})).unwrap()["result"], json!({}));
    }

    #[test]
    fn calls_a_tool() {
        let call = |arguments: Json| {
            ask(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "echo", "arguments": arguments}}))
                .unwrap()
        };
        let ok = call(json!({"text": "hi"}));
        assert_eq!(ok["result"]["content"][0]["text"], "\"hi\"");
        assert_eq!(ok["result"]["structuredContent"], json!({"text": "hi"}));
        assert_eq!(ok["result"]["isError"], false);
        // a tool's own error is a result, a bad call a protocol error
        assert_eq!(call(json!({"text": "boom"}))["result"]["isError"], true);
        assert_eq!(call(json!({}))["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn notifications_get_no_response_and_mistakes_get_errors() {
        assert_eq!(ask(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})), None);
        assert_eq!(
            ask(json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"})).unwrap()["error"]["code"],
            METHOD_NOT_FOUND
        );
        assert_eq!(ask(json!({"id": 1})).unwrap()["error"]["code"], INVALID_REQUEST);
        assert_eq!(handle_body(&mut Echo, b"{oops").unwrap()["error"]["code"], PARSE_ERROR);
    }
}
