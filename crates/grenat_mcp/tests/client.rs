//! The client against the fake server, over stdio (this test program, run
//! again as the server) and over HTTP. `harness = false`: `main` is ours.

use grenat_mcp::{Client, Http, Stdio, fake};
use serde_json::json;

fn exercise(mut client: Client) {
    assert_eq!(client.server, "fake");
    let tools = client.tools().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["echo", "add", "delete_all"]);
    assert!(tools[0].read_only && !tools[0].destructive);
    assert!(!tools[1].read_only && !tools[1].destructive);
    // no hints: may be destructive
    assert!(tools[2].destructive);
    assert_eq!(tools[1].input_schema["required"], json!(["a", "b"]));

    let echo = client.call("echo", json!({"text": "hi"})).unwrap();
    assert_eq!((echo.text.as_str(), echo.is_error), ("hi", false));
    assert_eq!(client.call("add", json!({"a": 2, "b": 3.5})).unwrap().text, "5.5");
    assert!(client.call("nope", json!({})).unwrap().is_error);
}

fn main() {
    if std::env::var_os("GRENAT_FAKE_MCP").is_some() {
        fake::serve_stdio();
        return;
    }
    let me = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    let stdio = Stdio::spawn(&[me], &[("GRENAT_FAKE_MCP".into(), "1".into())]).unwrap();
    exercise(Client::connect(Box::new(stdio)).unwrap());
    println!("stdio ... ok");

    let http = Http::new(&fake::serve_http(), &[("Authorization".into(), "Bearer t".into())], std::time::Duration::from_secs(5));
    exercise(Client::connect(Box::new(http)).unwrap());
    println!("http ... ok");

    let missing = Stdio::spawn(&["no-such-mcp-server".into()], &[]).err().unwrap();
    assert!(missing.starts_with("cannot start the MCP server `no-such-mcp-server`"), "{missing}");
    let refused = Client::connect(Box::new(Http::new("http://127.0.0.1:1/mcp", &[], std::time::Duration::from_secs(2)))).err().unwrap();
    assert!(refused.contains("127.0.0.1:1"), "{refused}");
    println!("errors ... ok");
}
