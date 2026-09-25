//! MCP: the servers the program uses (and their tools), and what it serves.

use super::{Ctx, Outcome, Result, page};
use crate::html::{badge, escape, table};

pub(crate) fn list(ctx: &mut Ctx) -> Result {
    let servers: Vec<Vec<String>> = ctx
        .app
        .mcp_servers
        .iter()
        .map(|s| vec![format!("<a href=\"/mcp/{0}\">{0}</a>", escape(&s.name)), format!("<code>{}</code>", escape(&s.target))])
        .collect();
    let exposed: Vec<Vec<String>> = ctx
        .app
        .exposures
        .iter()
        .map(|e| {
            vec![
                format!("<code>{}</code>", escape(&e.path)),
                escape(&e.tools.join(", ")),
                if e.public { badge("public", "warn") } else { badge("token", "ok") },
            ]
        })
        .collect();
    let body = format!(
        "<h2>Servers used</h2>{}<h2>Served (<code>expose</code>)</h2>{}",
        table(&["Server", "At"], &servers, "The program uses no MCP server."),
        table(&["Path", "Tools", "Access"], &exposed, "The program serves no tool."),
    );
    page("MCP", "/mcp", body)
}

pub(crate) fn tools(ctx: &mut Ctx, server: &str) -> Result {
    // only a server the program declares is connected to
    if !ctx.app.mcp_servers.iter().any(|s| s.name == server) {
        return Ok(Outcome::NotFound);
    }
    let body = match ctx.mcp.tools(server) {
        Ok(tools) => {
            let rows: Vec<Vec<String>> = tools
                .iter()
                .map(|t| {
                    let kind = if t.read_only { badge("read-only", "ok") } else if t.destructive { badge("may destroy", "bad") } else { badge("writes", "warn") };
                    vec![format!("<code>{}</code>", escape(&t.name)), escape(&t.description), kind]
                })
                .collect();
            table(&["Tool", "Description", ""], &rows, "This server offers no tool.")
        }
        Err(e) => format!("<p class=\"bad\">Could not list its tools: {}</p>", escape(&e)),
    };
    page(format!("MCP server {server}"), "/mcp", body)
}
