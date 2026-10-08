//! Triggers: what wakes a served program (`grenat serve`).
//!
//! ```ruby
//! every 1.week do … end                        # or: every cron: "0 8 * * MON" (UTC)
//! on_webhook "/github", secret: Env.fetch("GITHUB_WEBHOOK_SECRET"), signature: :github do |req|
//!   req.json["action"]                          # untrusted
//!   "ok"                                        # the response
//! end
//! on_webhook "/slack/events", signature: :slack, secret: Credentials.fetch(:slack, :signing_secret) do |req|
//!   …                                           # Slack's Events API, a slash command
//! end
//! on_webhook "/teams", signature: :teams, secret: Credentials.fetch(:teams, :security_token) do |req|
//!   {type: "message", text: "On it."}           # a Teams outgoing webhook's reply
//! end
//! ```
//!
//! A request whose signature (or token) does not match is refused (401)
//! before the handler runs: GitHub's (`X-Hub-Signature-256`), Slack's
//! (`X-Slack-Signature` of `v0:<timestamp>:<body>`, the timestamp within
//! five minutes of now, the same signature accepted once: a replay is
//! refused), Teams' (`Authorization: HMAC <base64>`, keyed by the security
//! token base64-decoded). Slack's `url_verification` — the challenge it
//! sends when the Events API's Request URL is set — is answered once
//! verified, without the handler. What a request carries is untrusted. In
//! tests, `deliver_webhook "/github", json: {…}` sends a request, signed as
//! it must be, through the same path.

use grenat_serve::Cron;

use crate::prelude::*;

use super::*;

/// When a schedule runs.
pub(crate) enum Every {
    Seconds(f64),
    Cron(Cron),
}

pub(crate) struct Schedule<'p> {
    pub every: Every,
    pub block: Value<'p>,
    pub label: String,
}

/// How a webhook proves where it comes from.
#[derive(Clone)]
pub(crate) enum Proof {
    None,
    /// `X-Hub-Signature-256`, as GitHub signs.
    Github(String),
    /// `X-Slack-Signature` and `X-Slack-Request-Timestamp`, as Slack signs.
    Slack(String),
    /// `Authorization: HMAC <base64>`, as a Teams outgoing webhook signs:
    /// the key, decoded.
    Teams(Vec<u8>),
    /// `Authorization: Bearer <token>`.
    Token(String),
}

pub(crate) struct Webhook<'p> {
    pub path: String,
    pub proof: Proof,
    pub block: Value<'p>,
}

pub(crate) fn every<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    let block = block(args, "every")?;
    let (every, label) = match (args.pos.first().map(Value::untainted), args.named.first()) {
        (Some(Value::Duration(s) | Value::Float(s)), None) if *s > 0.0 => (Every::Seconds(*s), format!("every {s}s")),
        (Some(Value::Int(n)), None) if *n > 0 => (Every::Seconds(*n as f64), format!("every {n}s")),
        (None, Some((name, Value::Str(text)))) if name == "cron" => {
            let cron = Cron::parse(text).or_else(|e| raise("ArgumentError", e))?;
            (Every::Cron(cron), format!("every cron: {text}"))
        }
        _ => return raise("ArgumentError", "`every` expects a duration (`every 1.hour`) or `cron: \"0 8 * * MON\"`"),
    };
    interp.schedules.borrow_mut().push(Schedule { every, block, label });
    Ok(Value::Nil)
}

/// What proves who calls a webhook.
const PROOFS: &str = "`secret:` with `signature: :github`, `:slack` or `:teams`, or `token:`";

pub(crate) fn on_webhook<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    let path = str_arg(args, 0, "on_webhook")?.to_string();
    if !path.starts_with('/') {
        return raise("ArgumentError", format!("a webhook path starts with `/`, got {path:?}"));
    }
    let named = |n: &str| args.named.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    if let Some((option, _)) = args.named.iter().find(|(k, _)| !matches!(k.as_str(), "signature" | "secret" | "token"))
    {
        return raise("ArgumentError", format!("`on_webhook` has no option `{option}:` ({PROOFS})"));
    }
    if args.named.iter().any(|(_, v)| v.contains_taint()) {
        return raise("TaintError", "an untrusted value proves who calls `on_webhook`: check it first");
    }
    let proof = match (named("signature").map(|s| s.untainted().clone()), named("secret"), named("token")) {
        (Some(Value::Symbol(kind)), Some(secret), None) => match &*kind {
            "github" => Proof::Github(secret.reveal()),
            "slack" => Proof::Slack(secret.reveal()),
            "teams" => match grenat_serve::signature::teams_key(&secret.reveal()) {
                Some(key) => Proof::Teams(key),
                // `Credentials.fetch` in a test without credentials: a stand-in, its bytes the key
                None if interp.offline && matches!(secret.untainted(), Value::Secret(_)) => {
                    Proof::Teams(secret.reveal().into_bytes())
                }
                None => {
                    return raise(
                        "ArgumentError",
                        "`signature: :teams` takes `secret:` the webhook's security token, as Teams shows it (base64)",
                    );
                }
            },
            other => return raise("ArgumentError", format!("`on_webhook` knows no `signature: :{other}` ({PROOFS})")),
        },
        (None, None, Some(token)) => Proof::Token(token.reveal()),
        (None, None, None) => Proof::None,
        _ => return raise("ArgumentError", format!("`on_webhook` takes {PROOFS}")),
    };
    if interp.slack_approvals.borrow().as_ref().is_some_and(|s| s.path == path) {
        return raise("ArgumentError", format!("`{path}` is the route of `approvals_in_slack`"));
    }
    let block = block(args, "on_webhook")?;
    interp.webhooks.borrow_mut().push(Webhook { path, proof, block });
    Ok(Value::Nil)
}

/// `deliver_webhook "/path", json: {…}` (or `body:`, `headers:`): the
/// response, as a hash (`status`, `body`).
pub(crate) fn deliver_webhook<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    let path = str_arg(args, 0, "deliver_webhook")?.to_string();
    let mut raw = RawRequest {
        method: "POST".into(),
        path: path.clone(),
        query: String::new(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    for (option, value) in &args.named {
        match (option.as_str(), value.untainted()) {
            ("json", v) => {
                raw.body = crate::llm::value_to_json(v).to_string().into_bytes();
                raw.headers.push(("Content-Type".into(), "application/json".into()));
            }
            ("body", v) => raw.body = v.to_display().into_bytes(),
            ("headers", Value::Hash(h)) => {
                raw.headers.extend(h.borrow().iter().map(|(k, v)| (k.to_display(), v.to_display())))
            }
            (option, v) => {
                return raise("ArgumentError", format!("invalid `deliver_webhook` option `{option}: {}`", v.inspect()));
            }
        }
    }
    // signed as the sender would sign it, unless the test gives its own
    let proof = interp.webhooks.borrow().iter().find(|w| w.path == path).map(|w| w.proof.clone());
    let given = |name: &str| raw.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name));
    match proof {
        Some(Proof::Github(secret)) if !given("X-Hub-Signature-256") => {
            let signature = grenat_serve::signature::github(&secret, &raw.body);
            raw.headers.push(("X-Hub-Signature-256".into(), signature));
        }
        Some(Proof::Slack(secret)) if !given("X-Slack-Signature") => {
            raw.headers.extend(interp.slack_headers(&secret, &raw.body));
        }
        Some(Proof::Teams(key)) if !given("Authorization") => {
            raw.headers.push(("Authorization".into(), grenat_serve::signature::teams(&key, &raw.body)));
        }
        Some(Proof::Token(token)) if !given("Authorization") => {
            raw.headers.push(("Authorization".into(), format!("Bearer {token}")))
        }
        _ => {}
    }
    let answer = interp.handle_webhook(raw)?;
    Ok(answer_value(answer, None))
}

impl<'p> Interp<'p> {
    /// Answers a webhook request. A request that does not prove where it
    /// comes from never reaches its handler.
    pub(crate) fn handle_webhook(&mut self, raw: RawRequest) -> Result<HttpAnswer<'p>, Ctrl<'p>> {
        let found =
            self.webhooks.borrow().iter().find(|w| w.path == raw.path).map(|w| (w.proof.clone(), w.block.clone()));
        let Some((proof, handler)) = found else { return Ok(HttpAnswer::text(404, "no such webhook")) };
        if raw.method != "POST" {
            return Ok(HttpAnswer::text(405, "webhooks are POST requests"));
        }
        let header =
            |name: &str| raw.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
        let signed = |valid: bool, why: &str| if valid { Ok(()) } else { Err(why.to_string()) };
        let authorization = header("Authorization");
        let proven = match &proof {
            Proof::None => Ok(()),
            Proof::Github(secret) => signed(
                header("X-Hub-Signature-256")
                    .is_some_and(|given| grenat_serve::signature::github_valid(secret, &raw.body, &given)),
                "bad signature",
            ),
            Proof::Slack(secret) => self.slack_signed(secret, &raw),
            Proof::Teams(key) => {
                signed(grenat_serve::signature::teams_valid(key, &raw.body, authorization.as_deref()), "bad signature")
            }
            Proof::Token(token) => {
                signed(grenat_serve::signature::bearer_valid(token, authorization.as_deref()), "bad token")
            }
        };
        if let Err(why) = proven {
            if self.log {
                self.write_err(&format!("[webhook] {}: refused ({why})\n", raw.path));
            }
            return Ok(HttpAnswer::text(401, "unauthorized"));
        }
        if let (Proof::Slack(_), Some(challenge)) = (&proof, url_verification(&raw.body)) {
            return Ok(HttpAnswer::text(200, &challenge));
        }
        let request = request_value(&raw, Vec::new());
        let answer = self.call_block(&handler, vec![request])?;
        Ok(answer_of(&answer))
    }
}

/// The challenge of Slack's `url_verification` (`{"type": "url_verification",
/// "challenge": "…"}`), answered as plain text.
fn url_verification(body: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    if json["type"] != "url_verification" {
        return None;
    }
    json["challenge"].as_str().filter(|c| c.len() <= 512).map(str::to_string)
}
