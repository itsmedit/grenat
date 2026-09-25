//! `grenat console`: the operations console of a Grenat application, in a
//! browser — approvals waiting for a human, jobs and their journals, what
//! the models cost, evals over time, failures and refusals, MCP servers.
//!
//! It observes and operates (approve, deny, retry); the code stays the
//! source of truth. Pages are HTML made on the server, without JavaScript.
//! Everything it shows comes from the operations store (`grenat_ops`) and
//! from what the program declares ([`Application`]); it knows nothing of
//! the language, nor of HTTP servers: [`Console::handle`] turns a
//! [`Request`] into a [`Response`].

mod auth;
mod form;
mod html;
mod pages;

use std::path::PathBuf;

pub use auth::{Access, MIN_TOKEN};
use auth::{Admission, Guard};
use grenat_db::Connection;

/// What the program declares, as the console shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Application {
    pub name: String,
    pub agents: Vec<Agent>,
    pub workflows: Vec<String>,
    pub tools: Vec<String>,
    /// `GET /tickets/:id`…
    pub routes: Vec<String>,
    pub mcp_servers: Vec<McpServer>,
    pub exposures: Vec<Exposure>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Agent {
    pub name: String,
    /// The messages it handles.
    pub handlers: Vec<String>,
}

/// An MCP server the program uses.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpServer {
    pub name: String,
    /// Its URL, or the command that runs it.
    pub target: String,
}

/// Tools and agents the program serves (`expose`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Exposure {
    pub path: String,
    pub tools: Vec<String>,
    pub public: bool,
}

/// Lists the tools of an MCP server the program uses (connecting to it).
pub trait Mcp {
    fn tools(&mut self, server: &str) -> Result<Vec<grenat_mcp::Tool>, String>;
}

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    fn page(status: u16, body: String) -> Response {
        Response { status, content_type: "text/html; charset=utf-8".into(), headers: Vec::new(), body }
    }

    /// See other: after a form, the page to show.
    fn redirect(to: &str) -> Response {
        Response { status: 303, content_type: "text/plain; charset=utf-8".into(), headers: vec![("Location".into(), to.into())], body: String::new() }
    }

    fn with_cookie(mut self, cookie: String) -> Response {
        self.headers.push(("Set-Cookie".into(), cookie));
        self
    }
}

/// Headers every answer carries: no script runs, no other site frames the
/// console, nothing is cached.
const SECURITY_HEADERS: [(&str, &str); 5] = [
    ("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; img-src data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'"),
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "DENY"),
    ("Referrer-Policy", "no-referrer"),
    ("Cache-Control", "no-store"),
];

pub struct Console {
    app: Application,
    journal_dir: PathBuf,
    guard: Guard,
}

impl Console {
    pub fn new(app: Application, journal_dir: PathBuf, access: Access) -> Console {
        Console { app, journal_dir, guard: Guard::new(access) }
    }

    /// Answers `request`, reading and acting on `db` (the application's
    /// database); `now` is seconds since the epoch.
    pub fn handle(&mut self, request: &Request, db: &mut dyn Connection, mcp: &mut dyn Mcp, now: f64) -> Response {
        let mut response = self.route(request, db, mcp, now);
        response.headers.extend(SECURITY_HEADERS.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        response
    }

    fn route(&mut self, request: &Request, db: &mut dyn Connection, mcp: &mut dyn Mcp, now: f64) -> Response {
        let post = request.method == "POST";
        let fields = if post { form::parse(&String::from_utf8_lossy(&request.body)) } else { Vec::new() };
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/login") if self.guard.needs_sign_in() => return Response::page(200, sign_in_page(None)),
            ("POST", "/login") if self.guard.needs_sign_in() => {
                return match self.guard.sign_in(&fields) {
                    Some(cookie) => Response::redirect("/").with_cookie(cookie),
                    None => Response::page(401, sign_in_page(Some("This token is not the console's."))),
                };
            }
            ("GET", "/login") => return Response::redirect("/"),
            _ => {}
        }
        let signed_in = match self.guard.admit(request) {
            Admission::Allowed { signed_in } => signed_in,
            Admission::Refused(response) => return response,
        };
        if post && !self.guard.form_is_ours(request, &fields) {
            return Response::page(403, html::bare("Forbidden", "<p>This form did not come from the console: reload the page and try again.</p>"));
        }
        if post && request.path == "/logout" {
            return Response::redirect("/login").with_cookie(self.guard.sign_out(request));
        }
        let mut ctx = pages::Ctx {
            db,
            mcp,
            app: &self.app,
            journal_dir: &self.journal_dir,
            now,
            query: form::parse(&request.query),
            csrf: self.guard.csrf(),
        };
        let segments: Vec<&str> = request.path.split('/').filter(|s| !s.is_empty()).collect();
        let outcome = pages::route(&mut ctx, &request.method, &segments);
        let pending = grenat_ops::approvals::pending(ctx.db).map(|p| p.len()).unwrap_or(0);
        let frame = |section| html::Frame { app: &self.app.name, section, pending, signed_in, csrf: self.guard.csrf() };
        match outcome {
            Ok(pages::Outcome::Page { title, section, body }) => Response::page(200, html::layout(&frame(section), &title, &body)),
            Ok(pages::Outcome::Redirect(to)) => Response::redirect(&to),
            Ok(pages::Outcome::NotFound) => Response::page(404, html::layout(&frame(""), "Not found", "<p>Nothing here.</p>")),
            Err(error) => {
                let body = format!("<p>The console could not read the application's data:</p><pre>{}</pre>", html::escape(&error));
                Response::page(500, html::layout(&frame(""), "Error", &body))
            }
        }
    }
}

fn sign_in_page(error: Option<&str>) -> String {
    let error = error.map(|e| format!("<p class=\"bad\">{}</p>", html::escape(e))).unwrap_or_default();
    html::bare(
        "Sign in",
        &format!(
            "{error}<form method=\"post\" action=\"/login\"><label for=\"token\">The console's token</label>\
             <input id=\"token\" name=\"token\" type=\"password\" autocomplete=\"current-password\" autofocus>\
             <p><button class=\"primary\">Sign in</button></p></form>"
        ),
    )
}
