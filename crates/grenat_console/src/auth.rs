//! Who may use the console. It acts — approves, retries — so:
//!
//! - listening on this machine only, it answers requests addressed to this
//!   machine (the `Host` header): a web page elsewhere cannot reach it by
//!   renaming a domain to 127.0.0.1 (DNS rebinding);
//! - anywhere else, it asks for a token once, then knows the browser by a
//!   session cookie (`HttpOnly`, `SameSite=Strict`);
//! - every form carries a secret of this process (CSRF), and a request that
//!   says it comes from another site (`Origin`) is refused.

use std::collections::HashSet;

use crate::{Request, Response, form, html};

/// The shortest token accepted: one a person cannot guess.
pub const MIN_TOKEN: usize = 16;
const COOKIE: &str = "grenat_console";

pub enum Access {
    /// Listening on this machine: requests must be addressed to `hosts`.
    Local { hosts: Vec<String> },
    /// Signing in with `token`.
    Token { token: String },
}

impl Access {
    /// The access for a console listening on `address` (`host:port`), with
    /// a token or not: a token is required off this machine.
    pub fn for_address(address: &str, token: Option<String>) -> Result<Access, String> {
        if let Some(token) = token {
            if token.chars().count() < MIN_TOKEN {
                return Err(format!("the console's token is too short: at least {MIN_TOKEN} characters"));
            }
            return Ok(Access::Token { token });
        }
        let (host, port) = address.rsplit_once(':').ok_or_else(|| format!("invalid address `{address}`"))?;
        if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
            return Err(format!(
                "the console acts on the application: listening on {address}, it needs a token (--token, or GRENAT_CONSOLE_TOKEN)"
            ));
        }
        let hosts = ["127.0.0.1", "localhost", "[::1]"].iter().map(|h| format!("{h}:{port}")).collect();
        Ok(Access::Local { hosts })
    }
}

pub struct Guard {
    access: Access,
    csrf: String,
    sessions: HashSet<String>,
}

/// What [`Guard::admit`] decided.
pub enum Admission {
    /// Let in; `signed_in` when by a session.
    Allowed {
        signed_in: bool,
    },
    Refused(Response),
}

impl Guard {
    pub fn new(access: Access) -> Guard {
        Guard { access, csrf: random(), sessions: HashSet::new() }
    }

    pub fn csrf(&self) -> &str {
        &self.csrf
    }

    pub fn needs_sign_in(&self) -> bool {
        matches!(self.access, Access::Token { .. })
    }

    /// Whether `request` may see pages (the sign-in page aside).
    pub fn admit(&self, request: &Request) -> Admission {
        match &self.access {
            Access::Local { hosts } => {
                let host = request.header("host").unwrap_or_default();
                if hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
                    Admission::Allowed { signed_in: false }
                } else {
                    Admission::Refused(Response::page(
                        403,
                        html::bare("Forbidden", "<p>This console answers requests addressed to this machine only.</p>"),
                    ))
                }
            }
            Access::Token { .. } => match session(request) {
                Some(id) if self.sessions.contains(id) => Admission::Allowed { signed_in: true },
                _ => Admission::Refused(Response::redirect("/login")),
            },
        }
    }

    /// Whether a form was posted from the console itself.
    pub fn form_is_ours(&self, request: &Request, fields: &[(String, String)]) -> bool {
        if let (Some(origin), Some(host)) = (request.header("origin"), request.header("host"))
            && origin != format!("http://{host}")
            && origin != format!("https://{host}")
        {
            return false;
        }
        same(form::get(fields, "csrf").unwrap_or_default(), &self.csrf)
    }

    /// Signs in with the token posted: the cookie that opens a session.
    pub fn sign_in(&mut self, fields: &[(String, String)]) -> Option<String> {
        let Access::Token { token } = &self.access else { return None };
        if !same(form::get(fields, "token").unwrap_or_default(), token) {
            return None;
        }
        let id = random();
        self.sessions.insert(id.clone());
        Some(format!("{COOKIE}={id}; Path=/; HttpOnly; SameSite=Strict"))
    }

    /// Ends the request's session: the cookie that forgets it.
    pub fn sign_out(&mut self, request: &Request) -> String {
        if let Some(id) = session(request) {
            self.sessions.remove(id);
        }
        format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
    }
}

/// The session id of the request's cookie.
fn session(request: &Request) -> Option<&str> {
    request.header("cookie")?.split(';').filter_map(|c| c.trim().strip_prefix(&format!("{COOKIE}="))).next()
}

/// Equality whose time does not tell how much of a secret was guessed.
fn same(given: &str, secret: &str) -> bool {
    given.len() == secret.len() && given.bytes().zip(secret.bytes()).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// 256 random bits, in hex.
fn random() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("the system's random numbers");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
