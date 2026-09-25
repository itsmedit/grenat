//! A fake MCP server, for tests: three tools (`echo`, read-only; `add`;
//! `delete_all`, destructive), listed over two pages; `fail` answers with a
//! tool error. Served over HTTP by [`serve_http`], or answered message by
//! message by [`handle`] (for a stdio server).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

use serde_json::{Value as Json, json};

/// The response to `message`, if it is a request.
pub fn handle(message: &Json) -> Option<Json> {
    let id = message.get("id")?.clone();
    let params = &message["params"];
    let result = match message["method"].as_str()? {
        "initialize" => json!({"protocolVersion": crate::PROTOCOL_VERSION, "capabilities": {"tools": {}}, "serverInfo": {"name": "fake"}}),
        "tools/list" if params.get("cursor").is_none() => json!({
            "tools": [
                {"name": "echo", "description": "Echoes a text.", "annotations": {"readOnlyHint": true},
                 "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
                {"name": "add", "description": "Adds two numbers.", "annotations": {"readOnlyHint": false, "destructiveHint": false},
                 "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}, "required": ["a", "b"]}},
            ],
            "nextCursor": "page2",
        }),
        "tools/list" => json!({"tools": [
            {"name": "delete_all", "description": "Deletes everything.",
             "inputSchema": {"type": "object", "properties": {}}},
        ]}),
        "tools/call" => {
            let args = &params["arguments"];
            match params["name"].as_str()? {
                "echo" => json!({"content": [{"type": "text", "text": args["text"]}]}),
                "add" => {
                    let sum = args["a"].as_f64().unwrap_or(0.0) + args["b"].as_f64().unwrap_or(0.0);
                    json!({"content": [{"type": "text", "text": sum.to_string()}]})
                }
                "delete_all" => json!({"content": [{"type": "text", "text": "deleted"}]}),
                _ => json!({"content": [{"type": "text", "text": "no such tool"}], "isError": true}),
            }
        }
        other => return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("unknown method {other}")}})),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// Serves stdin → stdout, one message per line, until the input ends.
pub fn serve_stdio() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if let Ok(message) = serde_json::from_str::<Json>(&line)
            && let Some(reply) = handle(&message)
        {
            let _ = writeln!(stdout, "{reply}");
            let _ = stdout.flush();
        }
    }
}

/// Serves over HTTP on a free port, answering with server-sent events;
/// returns the URL.
pub fn serve_http() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let url = format!("http://{}/mcp", listener.local_addr().expect("an address"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().expect("stream"));
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let message: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
                let (status, text) = match handle(&message) {
                    Some(reply) => ("200 OK", format!("event: message\ndata: {reply}\n\n")),
                    None => ("202 Accepted", String::new()),
                };
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nMcp-Session-Id: s1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            });
        }
    });
    url
}
