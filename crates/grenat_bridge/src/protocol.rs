//! JSON-RPC 2.0 as bridges speak it: one message per line (JSON never
//! holds a raw newline), requests from Grenat, responses from the server.
//!
//! ```text
//! → {"jsonrpc":"2.0","id":1,"method":"describe"}
//! ← {"jsonrpc":"2.0","id":1,"result":{"abi":1,"functions":[…],"structs":[…]}}
//! → {"jsonrpc":"2.0","id":2,"method":"call","params":{"name":"shout","args":["hi"]}}
//! ← {"jsonrpc":"2.0","id":2,"result":"HI"}
//! ← {"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"no such sheet","data":{"type":"SheetError"}}}
//! ```
//!
//! `describe` answers the manifest of a native library (`grenat_ext`), its
//! `abi` being this protocol's version; `call` runs a function with its
//! arguments (a JSON array, in order). An error raises the Grenat error its
//! `data.type` names if it ends in `Error`, else a `BridgeError`.

use serde_json::{Value as Json, json};

/// The version of the protocol: the `abi` a server's manifest declares.
pub const PROTOCOL_VERSION: u32 = 1;

/// The error type of what goes wrong in a bridge, unless the server names another.
pub const BRIDGE_ERROR: &str = "BridgeError";

/// The line of a request (without its newline).
pub fn request(id: u64, method: &str, params: Option<Json>) -> String {
    let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
    if let Some(params) = params {
        message["params"] = params;
    }
    message.to_string()
}

/// The params of `call`.
pub fn call(name: &str, args: Json) -> Json {
    json!({"name": name, "args": args})
}

/// What a request was answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Result(Json),
    Error(RpcError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    /// `data.type`: the Grenat error to raise.
    pub ty: Option<String>,
}

impl RpcError {
    /// The Grenat error to raise: the one the server named, if it is an
    /// error type's name (`SheetError`, which Grenat code can rescue), else
    /// a `BridgeError`.
    pub fn grenat_type(&self) -> String {
        match &self.ty {
            Some(ty) if grenat_native::declarations::is_error_name(ty) => ty.clone(),
            _ => BRIDGE_ERROR.to_string(),
        }
    }
}

/// A line of the server's standard output, as a response to the request
/// `id`; `None` if it is not one (a line the server should not have written).
pub fn response(line: &str) -> Option<(u64, Reply)> {
    let message: Json = serde_json::from_str(line).ok()?;
    if message.get("jsonrpc")? != "2.0" {
        return None;
    }
    let id = message.get("id")?.as_u64()?;
    if let Some(result) = message.get("result") {
        return Some((id, Reply::Result(result.clone())));
    }
    let error = message.get("error")?;
    let text = |key: &str| error.get(key).and_then(Json::as_str).map(str::to_string);
    Some((
        id,
        Reply::Error(RpcError {
            code: error.get("code").and_then(Json::as_i64).unwrap_or(-32603),
            message: text("message").unwrap_or_else(|| "the bridge answered an error without a message".into()),
            ty: error.get("data").and_then(|d| d.get("type")).and_then(Json::as_str).map(str::to_string),
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_lines_of_json_rpc() {
        let line: Json = serde_json::from_str(&request(1, "describe", None)).unwrap();
        assert_eq!(line, json!({"jsonrpc": "2.0", "id": 1, "method": "describe"}));
        let line = request(2, "call", Some(call("shout", json!(["a\nb"]))));
        assert!(!line.contains('\n'), "{line}");
        let back: Json = serde_json::from_str(&line).unwrap();
        assert_eq!(back["params"], json!({"name": "shout", "args": ["a\nb"]}));
    }

    #[test]
    fn responses_are_results_or_errors() {
        assert_eq!(response(r#"{"jsonrpc":"2.0","id":2,"result":"HI"}"#), Some((2, Reply::Result(json!("HI")))));
        assert_eq!(response(r#"{"jsonrpc":"2.0","id":3,"result":null}"#), Some((3, Reply::Result(Json::Null))));
        let Some((4, Reply::Error(e))) = response(
            r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32000,"message":"no sheet","data":{"type":"SheetError"}}}"#,
        ) else {
            panic!()
        };
        assert_eq!((e.code, e.message.as_str(), e.grenat_type()), (-32000, "no sheet", "SheetError".into()));
        let Some((5, Reply::Error(e))) = response(r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32601}}"#) else {
            panic!()
        };
        assert_eq!(e.grenat_type(), "BridgeError");
        // a type that is not a Grenat type name is not raised as such
        for ty in ["Error\nraise", "Refused", "sheetError"] {
            let e = RpcError { code: 0, message: String::new(), ty: Some(ty.into()) };
            assert_eq!(e.grenat_type(), "BridgeError", "{ty}");
        }
        for other in ["hello", "{}", r#"{"id":1,"result":1}"#, r#"{"jsonrpc":"2.0","id":"x","result":1}"#] {
            assert_eq!(response(other), None, "{other}");
        }
    }
}
